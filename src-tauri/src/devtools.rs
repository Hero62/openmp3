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
        "--api-probe" => rt.block_on(api_probe()),
        "--show-probe" => rt.block_on(show_probe(args.get(1).map(|s| s.as_str()).unwrap_or(""))),
        "--api-test" => rt.block_on(api_test(args.get(1).map(|s| s == "--edit").unwrap_or(false))),
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

/// Hits each internal endpoint once and prints a one-line summary.
pub async fn api_probe() -> Result<()> {
    use engine_api::endpoints::Endpoints;
    let mgr = SessionManager::new(AppPaths::resolve(), None)?;
    let session = connect(&mgr).await?;
    let ep = Endpoints::new(session);
    macro_rules! probe {
        ($name:expr, $e:expr) => {{
            let t = Instant::now();
            match $e {
                Ok(s) => eprintln!("[probe] OK   {:<18} {:>5} ms  {}", $name, t.elapsed().as_millis(), s),
                Err(e) => eprintln!("[probe] FAIL {:<18} {:>5} ms  {:#}", $name, t.elapsed().as_millis(), e),
            }
        }};
    }
    let mut first_playlist = None;
    probe!("rootlist", ep.rootlist().await.map(|r| {
        let items = r.contents.items.len();
        first_playlist = r.contents.items.iter().filter_map(|i| i.uri.clone()).find(|u| u.starts_with("spotify:playlist:"));
        let names: Vec<String> = r.contents.meta_items.iter().take(3).filter_map(|m| m.attributes.name.clone()).collect();
        format!("{items} items, first: {names:?}")
    }));
    let mut first_tracks: Vec<String> = Vec::new();
    if let Some(p) = &first_playlist {
        let b62 = p.rsplit(':').next().unwrap().to_string();
        probe!("playlist", ep.playlist(&b62).await.map(|c| {
            first_tracks = c.contents.items.iter().filter_map(|i| i.uri.clone()).filter(|u| u.starts_with("spotify:track:")).take(5).collect();
            format!("{:?}: {} items", c.attributes.name, c.contents.items.len())
        }));
    }
    if first_tracks.is_empty() {
        first_tracks.push(TEST_TRACK.to_string());
    }
    probe!("tracks_meta", ep.tracks_meta(&first_tracks).await.map(|v| {
        v.iter().map(|(_, t)| t.name.clone().unwrap_or_default()).collect::<Vec<_>>().join(" | ")
    }));
    let mut liked = Vec::new();
    probe!("collection", ep.collection("collection").await.map(|v| {
        liked = v.clone();
        let tracks = v.iter().filter(|(u, _)| u.starts_with("spotify:track:")).count();
        let albums = v.iter().filter(|(u, _)| u.starts_with("spotify:album:")).count();
        format!("{tracks} tracks, {albums} albums")
    }));
    probe!("collection:artist", ep.collection("artist").await.map(|v| format!("{} followed artists", v.len())));
    probe!("contains", ep.collection_contains("collection", &first_tracks).await.map(|v| format!("{v:?}")));
    probe!("context liked", ep.context(&format!("spotify:user:{}:collection", ep.username())).await.map(|c| {
        format!("{} pages, first page {} tracks", c.pages.len(), c.pages.first().map(|p| p.tracks.len()).unwrap_or(0))
    }));
    probe!("albums_meta", ep.albums_meta(&["spotify:album:6dVIqQ8qmQ5GBnJ9shOYGE".into()]).await.map(|v| {
        v.first().map(|(_, a)| format!("{:?} discs={}", a.name, a.disc.len())).unwrap_or_default()
    }));
    probe!("artists_meta", ep.artists_meta(&["spotify:artist:0gxyHStUsqpMadRV0Di1Qt".into()]).await.map(|v| {
        v.first().map(|(_, a)| format!("{:?} top={} albums={}", a.name, a.top_track.len(), a.album_group.len())).unwrap_or_default()
    }));
    probe!("lyrics", ep.lyrics("4cOdK2wGLETKBW3PvgPWqT").await.map(|v| {
        v.map(|v| format!("syncType={} lines={}", v["lyrics"]["syncType"], v["lyrics"]["lines"].as_array().map(|a| a.len()).unwrap_or(0))).unwrap_or("none".into())
    }));
    probe!("radio", ep.radio_for(TEST_TRACK).await.map(|v| format!("{v:?}")));
    probe!("autoplay", ep.autoplay("spotify:album:6dVIqQ8qmQ5GBnJ9shOYGE", &[TEST_TRACK.to_string()]).await.map(|c| {
        format!("{:?} pages={} tracks={}", c.uri, c.pages.len(), c.pages.first().map(|p| p.tracks.len()).unwrap_or(0))
    }));
    probe!("search", ep.search("daft punk", 5).await.map(|v| {
        let t = v["data"]["searchV2"]["tracksV2"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
        let first = v["data"]["searchV2"]["tracksV2"]["items"][0]["item"]["data"]["name"].clone();
        format!("{t} tracks, first {first}")
    }));
    probe!("home", ep.home("America/Los_Angeles").await.map(|v| {
        let secs = v["data"]["home"]["sectionContainer"]["sections"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
        let titles: Vec<String> = v["data"]["home"]["sectionContainer"]["sections"]["items"].as_array().map(|a| a.iter().take(4).map(|s| s["data"]["title"]["transformedLabel"].as_str().unwrap_or("?").to_string()).collect()).unwrap_or_default();
        format!("{secs} sections {titles:?}")
    }));
    probe!("artist_overview", ep.artist_overview("spotify:artist:0gxyHStUsqpMadRV0Di1Qt").await.map(|v| {
        format!("{} monthly={} related={}", v["data"]["artistUnion"]["profile"]["name"], v["data"]["artistUnion"]["stats"]["monthlyListeners"], v["data"]["artistUnion"]["relatedContent"]["relatedArtists"]["items"].as_array().map(|a| a.len()).unwrap_or(0))
    }));
    probe!("recently_played", ep.recently_played().await.map(|v| format!("{} contexts", v["playContexts"].as_array().map(|a| a.len()).unwrap_or(0))));
    probe!("show eps", ep.show_episode_uris("spotify:show:4rOoJ6Egrf8K2IrywzwOMk").await.map(|v| format!("{} episodes", v.len())));
    let _ = liked;
    Ok(())
}

/// Exercises engine-api end to end (refresh + cached read timings).
pub async fn api_test(edit: bool) -> Result<()> {
    use engine_api::{keys, models::*, Api};
    let paths = AppPaths::resolve();
    let mgr = SessionManager::new(paths.clone(), None)?;
    let session = connect(&mgr).await?;
    let api = Api::open(&paths.db_path())?;
    api.attach(session).await;
    macro_rules! t {
        ($name:expr, $e:expr, $fmt:expr) => {{
            let t = Instant::now();
            match $e {
                Ok(v) => { eprintln!("[api] OK   {:<16} {:>6} ms  {}", $name, t.elapsed().as_millis(), $fmt(&v)); Some(v) }
                Err(e) => { eprintln!("[api] FAIL {:<16} {:>6} ms  {:#}", $name, t.elapsed().as_millis(), e); None }
            }
        }};
    }
    let pls = t!("playlists", api.refresh_playlists().await, |v: &(Vec<PlaylistSummary>, bool)| format!("{} playlists", v.0.len()));
    if let Some((pls, _)) = &pls {
        if let Some(p) = pls.iter().max_by_key(|p| p.track_count) {
            t!("playlist", api.refresh_playlist(&p.uri).await, |v: &(Playlist, bool)| format!("{:?} {} tracks, first {:?}", v.0.summary.name, v.0.tracks.len(), v.0.tracks.first().map(|t| &t.title)));
        }
    }
    t!("liked", api.refresh_liked().await, |v: &(Vec<Track>, bool)| format!("{} tracks, newest {:?} by {:?}", v.0.len(), v.0.first().map(|t| &t.title), v.0.first().and_then(|t| t.artists.first()).map(|a| &a.name)));
    t!("liked (2nd)", api.refresh_liked().await, |v: &(Vec<Track>, bool)| format!("{} tracks changed={}", v.0.len(), v.1));
    t!("albums", api.refresh_albums().await, |v: &(Vec<AlbumRef>, bool)| format!("{:?}", v.0.iter().map(|a| &a.name).collect::<Vec<_>>()));
    t!("artists", api.refresh_artists().await, |v: &(Vec<ArtistRef>, bool)| format!("{} artists", v.0.len()));
    t!("shows", api.refresh_shows().await, |v: &(Vec<AlbumRef>, bool)| format!("{} shows", v.0.len()));
    t!("album", api.refresh_album("spotify:album:6dVIqQ8qmQ5GBnJ9shOYGE").await, |v: &(Album, bool)| format!("{} ({:?}) {} tracks, {}", v.0.name, v.0.release_year, v.0.tracks.len(), v.0.album_type));
    t!("artist", api.refresh_artist("spotify:artist:0gxyHStUsqpMadRV0Di1Qt").await, |v: &(Artist, bool)| format!("{} top={} albums={} singles={} related={} imgs={}", v.0.name, v.0.top_tracks.len(), v.0.albums.len(), v.0.singles.len(), v.0.related.len(), v.0.images.len()));
    t!("search", api.search("radiohead", 10).await, |v: &SearchResults| format!("t={} al={} ar={} pl={} sh={}", v.tracks.len(), v.albums.len(), v.artists.len(), v.playlists.len(), v.shows.len()));
    t!("home", api.refresh_home("America/Los_Angeles").await, |v: &(Vec<HomeSection>, bool)| format!("{} sections: {:?}", v.0.len(), v.0.iter().take(6).map(|s| format!("{} [{}:{}]", s.title, s.kind, s.items.len())).collect::<Vec<_>>()));
    t!("lyrics", api.spotify_lyrics(TEST_TRACK).await, |v: &Option<Lyrics>| v.as_ref().map(|l| format!("{} lines synced={} via {}", l.lines.len(), l.synced, l.source)).unwrap_or("none".into()));
    t!("autoplay", api.autoplay(Some("spotify:album:6dVIqQ8qmQ5GBnJ9shOYGE"), &[TEST_TRACK.into()]).await, |v: &Vec<Track>| format!("{} tracks, first {:?}", v.len(), v.first().map(|t| &t.title)));
    t!("recommend", api.recommendations(&[TEST_TRACK.into()]).await, |v: &Vec<Track>| format!("{} tracks", v.len()));
    t!("context liked", api.context_tracks("spotify:collection:tracks").await, |v: &(String, Vec<Track>)| format!("{} {}", v.0, v.1.len()));
    // Cached reads (what cold start uses).
    let t0 = Instant::now();
    let liked: Option<(Vec<Track>, i64)> = api.cached(keys::LIKED);
    let pl: Option<(Vec<PlaylistSummary>, i64)> = api.cached(keys::PLAYLISTS);
    let home: Option<(Vec<HomeSection>, i64)> = api.cached(keys::HOME);
    eprintln!("[api] CACHED liked={} playlists={} home={} in {} µs", liked.map(|l| l.0.len()).unwrap_or(0), pl.map(|l| l.0.len()).unwrap_or(0), home.map(|l| l.0.len()).unwrap_or(0), t0.elapsed().as_micros());

    if edit {
        let name = "mp3palace test (delete me)";
        let Some(uri) = t!("pl create", api.playlist_create(name).await, |u: &String| u.clone()) else { return Ok(()) };
        t!("pl rename", api.playlist_rename(&uri, "mp3palace test (renamed)").await, |_: &()| "ok".to_string());
        let tracks = vec![TEST_TRACK.to_string(), "spotify:track:0VjIjW4GlUZAMYd2vXMi3b".into(), "spotify:track:7qiZfU4dY1lWllzX7mPBI3".into()];
        t!("pl add", api.playlist_add(&uri, &tracks, None).await, |_: &()| "ok".to_string());
        t!("pl move", api.playlist_move(&uri, 2, 1, 0).await, |_: &()| "ok".to_string());
        t!("pl remove", api.playlist_remove(&uri, vec![1]).await, |_: &()| "ok".to_string());
        t!("pl verify", api.refresh_playlist(&uri).await, |v: &(Playlist, bool)| format!("{:?}: {:?}", v.0.summary.name, v.0.tracks.iter().map(|t| &t.title).collect::<Vec<_>>()));
        t!("pl delete", api.playlist_delete(&uri).await, |_: &()| "ok".to_string());
        t!("pl gone", api.refresh_playlists().await, |v: &(Vec<PlaylistSummary>, bool)| format!("still listed: {}", v.0.iter().any(|p| p.uri == uri)));
    }
    Ok(())
}

pub async fn show_probe(uri: &str) -> Result<()> {
    use engine_api::endpoints::Endpoints;
    let mgr = SessionManager::new(AppPaths::resolve(), None)?;
    let ep = Endpoints::new(connect(&mgr).await?);
    let assoc = ep.show_episode_uris(uri).await?;
    eprintln!("[show] assoc: {} uris, first {:?} last {:?}", assoc.len(), assoc.first(), assoc.last());
    let ctx = ep.context(uri).await?;
    eprintln!("[show] context pages={} first page tracks={} next={:?}", ctx.pages.len(), ctx.pages.first().map(|p| p.tracks.len()).unwrap_or(0), ctx.pages.first().and_then(|p| p.next_page_url.clone()));
    for p in ctx.pages.iter().take(3) {
        eprintln!("[show]  page tracks={} first={:?} last={:?} next={:?}", p.tracks.len(), p.tracks.first().and_then(|t| t.uri.clone()), p.tracks.last().and_then(|t| t.uri.clone()), p.next_page_url);
    }
    let eps = ep.episodes_meta(&[assoc.first().cloned().unwrap_or_default(), assoc.last().cloned().unwrap_or_default()]).await?;
    for (u, e) in eps {
        eprintln!("[show] {u} = {:?} published {:?}", e.name, e.publish_time.as_ref().map(|d| (d.year, d.month, d.day)));
    }
    Ok(())
}
