//! Headless developer commands (no window), e.g.
//! `mp3palace.exe --play-test [spotify:track:...] [seconds]`.

use std::time::{Duration, Instant};

use anyhow::Result;
use engine_audio::{AudioConfig, AudioEngine, PlayerEvent};
use engine_common::AppPaths;
use engine_session::SessionManager;

/// "Never Gonna Give You Up" — stable, available in every market.
pub const TEST_TRACK: &str = "spotify:track:4cOdK2wGLETKBW3PvgPWqT";

/// Returns Some(exit code) if a dev command was handled.
pub fn run_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first()?.as_str();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().ok()?;
    let res = match cmd {
        "--play-test" => {
            let uri = args.get(1).cloned().unwrap_or_else(|| TEST_TRACK.to_string());
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20u64);
            rt.block_on(play_test(&uri, secs))
        }
        "--login" => rt.block_on(login_only()),
        "--engine-test" => rt.block_on(engine_test()),
        _ => return None,
    };
    Some(match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[dev] error: {e:#}");
            1
        }
    })
}

async fn connect(mgr: &SessionManager) -> Result<engine_session::librespot_core::Session> {
    if let Some(s) = mgr.connect_cached().await? {
        eprintln!("[dev] connected from cached credentials as {}", s.username());
        return Ok(s);
    }
    eprintln!("[dev] no cached credentials — opening browser for Spotify login (port {})", engine_session::OAUTH_PORT);
    let s = mgr.login_oauth().await?;
    eprintln!("[dev] logged in as {}", s.username());
    Ok(s)
}

async fn login_only() -> Result<()> {
    let mgr = SessionManager::new(AppPaths::resolve(), None)?;
    connect(&mgr).await?;
    eprintln!("[dev] credentials cached: {}", mgr.has_cached_credentials());
    Ok(())
}

async fn play_test(uri: &str, secs: u64) -> Result<()> {
    let paths = AppPaths::resolve();
    let mgr = SessionManager::new(paths, Some(1024 * 1024 * 1024))?;
    let session = connect(&mgr).await?;

    let mut engine = AudioEngine::new(AudioConfig::default())?;
    engine.attach_session(&session);
    let mut events = engine.events(0).expect("deck 0 player");
    let started = Instant::now();
    engine.load(0, uri, true, 0)?;
    eprintln!("[dev] loading {uri}");

    let shared = engine.shared.clone();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut first_audio: Option<Duration> = None;
    let mut max_peak = 0f32;
    let deadline = started + Duration::from_secs(secs);
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(PlayerEvent::Playing { position_ms, .. }) => eprintln!("[dev] event Playing at {position_ms} ms (+{} ms since load)", started.elapsed().as_millis()),
                Some(PlayerEvent::Loading { .. }) => eprintln!("[dev] event Loading"),
                Some(PlayerEvent::Unavailable { .. }) => anyhow::bail!("track unavailable"),
                Some(PlayerEvent::EndOfTrack { .. }) => { eprintln!("[dev] event EndOfTrack"); break; }
                Some(_) => {}
                None => anyhow::bail!("player event channel closed"),
            },
            _ = tick.tick() => {
                let peak = shared.last_peak.get();
                let rms = shared.last_rms.get();
                max_peak = max_peak.max(peak);
                if first_audio.is_none() && peak > 0.001 {
                    first_audio = Some(started.elapsed());
                }
                eprintln!("[dev] t={:>4.1}s peak={peak:.3} rms={rms:.3} frames_out={}",
                    started.elapsed().as_secs_f32(), shared.frames_out.load(std::sync::atomic::Ordering::Relaxed));
                if Instant::now() >= deadline { break; }
            }
        }
    }
    eprintln!("[dev] RESULT max_peak={max_peak:.3} first_audio={:?}", first_audio);
    if max_peak < 0.001 {
        anyhow::bail!("no audio reached the output");
    }
    Ok(())
}

fn test_track(uri: &str) -> engine_api::models::Track {
    engine_api::models::Track { uri: uri.into(), title: uri.rsplit(':').next().unwrap_or("").into(), ..Default::default() }
}

/// Exercises the playback controller end to end against the real account:
/// gapless transition, crossfade transition, skip, prev and persistence.
pub async fn engine_test() -> Result<()> {
    use crate::playback::{self, Cmd, Event};
    use engine_queue::{store::StateStore, ContextInfo};
    use std::sync::Arc;

    let paths = AppPaths::resolve();
    let mgr = SessionManager::new(paths.clone(), Some(1024 * 1024 * 1024))?;
    let session = connect(&mgr).await?;
    let store = Arc::new(StateStore::open(&paths.cache.join("engine-test-state.sqlite"))?);
    let audio = AudioEngine::new(AudioConfig::default())?;
    let pb = playback::spawn(audio, store.clone());
    let mut ev = pb.subscribe();
    pb.send(Cmd::AttachSession(session));
    pb.send(Cmd::SetVolume(0.6));

    let tracks: Vec<_> = ["spotify:track:4cOdK2wGLETKBW3PvgPWqT", "spotify:track:0VjIjW4GlUZAMYd2vXMi3b", "spotify:track:7qiZfU4dY1lWllzX7mPBI3"]
        .iter()
        .map(|u| test_track(u))
        .collect();
    let t0 = Instant::now();
    let log = |m: String| eprintln!("[test {:>6.2}s] {m}", t0.elapsed().as_secs_f32());

    // Wait for an event matching `f`, up to `secs`.
    async fn wait_for(
        ev: &mut tokio::sync::broadcast::Receiver<Event>,
        secs: u64,
        mut f: impl FnMut(&Event) -> bool,
    ) -> Result<Event> {
        let dl = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            match tokio::time::timeout_at(dl, ev.recv()).await {
                Ok(Ok(e)) if f(&e) => return Ok(e),
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(e)) => anyhow::bail!("event channel: {e}"),
                Err(_) => anyhow::bail!("timeout"),
            }
        }
    }
    let playing = |e: &Event| matches!(e, Event::PlayStateChanged(s) if s.playing && !s.loading);

    // 1. gapless: crossfade off, seek near end, measure EndOfTrack -> next playing.
    pb.send(Cmd::SetCrossfade(0));
    pb.send(Cmd::PlayContext { info: ContextInfo { uri: "test:ctx".into(), name: "test".into() }, tracks: tracks.clone(), start: Some(0) });
    wait_for(&mut ev, 15, playing).await?;
    log(format!("playing {}", pb.state().track.unwrap().uri));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let dur = pb.state().duration_ms;
    anyhow::ensure!(dur > 60_000, "duration not learned ({dur})");
    pb.send(Cmd::Seek(dur - 5_000));
    log(format!("seek to {} of {dur}", dur - 5000));
    let changed = wait_for(&mut ev, 15, |e| matches!(e, Event::TrackChanged(_))).await?;
    let t_change = Instant::now();
    if let Event::TrackChanged(s) = changed {
        log(format!("gapless -> {}", s.track.unwrap().uri));
    }
    wait_for(&mut ev, 10, playing).await?;
    let gap = t_change.elapsed();
    log(format!("GAPLESS track-change -> playing: {} ms", gap.as_millis()));

    // 2. crossfade 4 s.
    pb.send(Cmd::SetCrossfade(4_000));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let dur = pb.state().duration_ms;
    pb.send(Cmd::Seek(dur - 7_000));
    wait_for(&mut ev, 15, |e| matches!(e, Event::TrackChanged(_))).await?;
    log(format!("crossfade started -> {}", pb.state().track.unwrap().uri));
    wait_for(&mut ev, 10, playing).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    log("CROSSFADE ok".into());

    // 3. prev + skip + queue
    pb.send(Cmd::SetCrossfade(0));
    pb.send(Cmd::AddToQueue(test_track("spotify:track:4cOdK2wGLETKBW3PvgPWqT")));
    pb.send(Cmd::Next);
    let e = wait_for(&mut ev, 10, |e| matches!(e, Event::TrackChanged(_))).await?;
    if let Event::TrackChanged(s) = e {
        anyhow::ensure!(s.track.unwrap().uri.ends_with("4cOdK2wGLETKBW3PvgPWqT"), "user queue did not play first");
        log("SKIP to user-queued track ok".into());
    }
    wait_for(&mut ev, 10, playing).await?;
    pb.send(Cmd::Prev);
    let e = wait_for(&mut ev, 10, |e| matches!(e, Event::TrackChanged(_))).await?;
    if let Event::TrackChanged(s) = e {
        log(format!("PREV -> {}", s.track.unwrap().uri));
    }
    wait_for(&mut ev, 10, playing).await?;
    pb.send(Cmd::Pause);
    wait_for(&mut ev, 5, |e| matches!(e, Event::PlayStateChanged(s) if !s.playing)).await?;
    log("PAUSE ok".into());
    tokio::time::sleep(Duration::from_secs(3)).await; // let persistence tick
    let q: Option<engine_queue::Queue> = store.load("queue");
    anyhow::ensure!(q.map(|q| q.current.is_some()).unwrap_or(false), "queue not persisted");
    log("PERSIST ok".into());
    Ok(())
}
