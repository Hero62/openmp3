//! Spotify Connect device (shows up on the phone as "openmp3").

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

use log::{info, warn};
use tauri::AppHandle;

use crate::{app::State, playback::Cmd};

static STARTED: AtomicBool = AtomicBool::new(false);

pub async fn start(_app: &AppHandle, state: &State, _session: engine_session::librespot_core::Session) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run(&state).await {
            warn!("Spotify Connect unavailable: {e:#}");
            STARTED.store(false, Ordering::SeqCst);
        }
    });
}

async fn run(state: &State) -> anyhow::Result<()> {
    let (session, creds) = state.session.connect_session()?;
    let pb = state.pb_wait().await.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    pb.send(Cmd::CreateConnectPlayer(session.clone(), tx));
    let player = rx.await?.ok_or_else(|| anyhow::anyhow!("no connect deck"))?;
    let pb2 = pb.clone();
    let mixer = Arc::new(engine_connect::EngineMixer::new(
        pb.state().volume,
        Arc::new(move |v| pb2.send(Cmd::RemoteVolume(v))),
    ));
    let (connect, task) = engine_connect::start(engine_common::APP_NAME, session, creds, player, mixer).await?;
    pb.send(Cmd::SetRemote(connect.clone()));
    info!("Spotify Connect device registered as \"{}\"", engine_common::APP_NAME);
    task.await;
    warn!("Spotify Connect task ended");
    Ok(())
}
