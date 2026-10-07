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
                Some(PlayerEvent::Playing { position_ms, .. }) => eprintln!("[dev] event Playing at {position_ms} ms"),
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
