//! Playback controller: owns the queue and drives the audio engine.
//!
//! Runs as one tokio task. Commands come in over an mpsc channel; state and
//! events go out over a broadcast channel. Handles gapless preloading,
//! crossfades between the two decks, repeat/shuffle, persistence and
//! autoplay hand-off (asks the outside world for more tracks when the queue
//! runs dry).

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use engine_api::models::Track;
use engine_audio::{dsp::EqSettings, AudioEngine, PlayerEvent};
use engine_queue::{store::StateStore, ContextInfo, Entry, Queue, QueueView, RepeatMode, ShuffleMode};
use engine_session::librespot_core::Session;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

const QUEUE_KEY: &str = "queue";
const PREFS_KEY: &str = "playback_prefs";

pub enum Cmd {
    AttachSession(Session),
    PlayContext { info: ContextInfo, tracks: Vec<Track>, start: Option<usize> },
    PlayTrack(Track),
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
    Seek(u32),
    SetVolume(f32),
    SetShuffle(ShuffleMode),
    SetRepeat(RepeatMode),
    AddToQueue(Track),
    PlayNext(Track),
    RemoveFromQueue(u64),
    MoveInQueue { uid: u64, to: usize },
    ClearQueue,
    SetEq(EqSettings),
    SetCrossfade(u32),
    /// Autoplay results for the context that just ended.
    ExtendContext(Vec<Track>),
    /// Create the Spotify Connect deck's player on the Connect session.
    CreateConnectPlayer(Session, tokio::sync::oneshot::Sender<Option<Arc<engine_audio::librespot_playback::player::Player>>>),
    /// Connect is up: route controls here while the phone is in charge.
    SetRemote(Arc<dyn engine_connect::RemoteControl>),
    /// Volume changed from the Connect side: apply locally without echoing back.
    RemoteVolume(f32),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub track: Option<Track>,
    pub playing: bool,
    pub loading: bool,
    pub position_ms: u32,
    /// Unix ms when `position_ms` was sampled, so UIs can interpolate.
    pub position_at: u64,
    pub duration_ms: u32,
    pub volume: f32,
    pub shuffle: ShuffleMode,
    pub repeat: RepeatMode,
    pub crossfade_ms: u32,
    pub context: Option<ContextInfo>,
    /// True while Spotify Connect (e.g. the phone) is driving playback.
    #[serde(default)]
    pub remote: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    TrackChanged(PlaybackState),
    PlayStateChanged(PlaybackState),
    Progress { position_ms: u32, duration_ms: u32 },
    QueueChanged(QueueView),
    /// Queue ran out; the host should fetch autoplay tracks for this seed.
    NeedAutoplay { seed_uris: Vec<String>, context_uri: Option<String> },
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prefs {
    volume: f32,
    crossfade_ms: u32,
    eq: EqSettings,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { volume: 0.8, crossfade_ms: 0, eq: EqSettings::default() }
    }
}

#[derive(Clone)]
pub struct PlaybackHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    events: broadcast::Sender<Event>,
    state: Arc<std::sync::RwLock<PlaybackState>>,
    queue_view: Arc<std::sync::RwLock<QueueView>>,
    eq: Arc<std::sync::RwLock<EqSettings>>,
}

impl PlaybackHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    pub fn state(&self) -> PlaybackState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn queue(&self) -> QueueView {
        self.queue_view.read().unwrap().clone()
    }
    pub fn eq(&self) -> EqSettings {
        self.eq.read().unwrap().clone()
    }
}

struct Deck {
    /// Request id of the load we issued (learned from PlayRequestIdChanged).
    request_id: Option<u64>,
    awaiting_id: bool,
    uri: Option<String>,
}

struct Controller {
    audio: AudioEngine,
    queue: Queue,
    store: Arc<StateStore>,
    prefs: Prefs,
    state: PlaybackState,
    decks: [Deck; 2],
    active: usize,
    /// When the active deck's position was last reported.
    pos_sample: (u32, Instant),
    crossfading: bool,
    preloaded_for: Option<u64>,
    events: broadcast::Sender<Event>,
    shared_state: Arc<std::sync::RwLock<PlaybackState>>,
    shared_queue: Arc<std::sync::RwLock<QueueView>>,
    shared_eq: Arc<std::sync::RwLock<EqSettings>>,
    autoplay_pending: bool,
    dirty_queue: bool,
    pending_stop: Option<(usize, Instant)>,
    remote: Option<Arc<dyn engine_connect::RemoteControl>>,
    remote_active: bool,
    remote_req: Option<u64>,
    remote_track: Option<(Track, u32)>,
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn spawn(audio: AudioEngine, store: Arc<StateStore>) -> PlaybackHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(256);
    let queue: Queue = store.load(QUEUE_KEY).unwrap_or_default();
    let prefs: Prefs = store.load(PREFS_KEY).unwrap_or_default();
    audio.set_volume(prefs.volume);
    audio.set_eq(prefs.eq.clone());
    let state = PlaybackState {
        track: queue.current.as_ref().map(|e| e.track.clone()),
        duration_ms: queue.current.as_ref().map(|e| e.track.duration_ms).unwrap_or(0),
        volume: prefs.volume,
        shuffle: queue.shuffle,
        repeat: queue.repeat,
        crossfade_ms: prefs.crossfade_ms,
        context: queue.context.clone(),
        position_at: unix_ms(),
        ..Default::default()
    };
    let shared_state = Arc::new(std::sync::RwLock::new(state.clone()));
    let shared_queue = Arc::new(std::sync::RwLock::new(queue.view(50)));
    let shared_eq = Arc::new(std::sync::RwLock::new(prefs.eq.clone()));
    let handle = PlaybackHandle {
        tx,
        events: events.clone(),
        state: shared_state.clone(),
        queue_view: shared_queue.clone(),
        eq: shared_eq.clone(),
    };
    let ctl = Controller {
        audio,
        queue,
        store,
        prefs,
        state,
        decks: [
            Deck { request_id: None, awaiting_id: false, uri: None },
            Deck { request_id: None, awaiting_id: false, uri: None },
        ],
        active: 0,
        pos_sample: (0, Instant::now()),
        crossfading: false,
        preloaded_for: None,
        events,
        shared_state,
        shared_queue,
        shared_eq,
        autoplay_pending: false,
        dirty_queue: false,
        pending_stop: None,
        remote: None,
        remote_active: false,
        remote_req: None,
        remote_track: None,
    };
    tokio::spawn(ctl.run(rx));
    handle
}

impl Controller {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        let mut ev0: Option<engine_audio::PlayerEventChannel> = None;
        let mut ev1: Option<engine_audio::PlayerEventChannel> = None;
        let mut ev2: Option<engine_audio::PlayerEventChannel> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut persist = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    if let Cmd::AttachSession(s) = &cmd {
                        self.audio.attach_session(s);
                        ev0 = self.audio.events(0);
                        ev1 = self.audio.events(1);
                        continue;
                    }
                    if let Cmd::CreateConnectPlayer(s, reply) = cmd {
                        let p = self.audio.create_connect_player(&s);
                        ev2 = self.audio.events(engine_audio::CONNECT_DECK);
                        let _ = reply.send(p);
                        continue;
                    }
                    self.handle(cmd);
                }
                Some(e) = recv_opt(&mut ev0) => self.on_player_event(0, e),
                Some(e) = recv_opt(&mut ev1) => self.on_player_event(1, e),
                Some(e) = recv_opt(&mut ev2) => self.on_connect_event(e),
                _ = tick.tick() => self.on_tick(),
                _ = persist.tick() => self.persist(),
            }
        }
        self.persist();
    }

    fn handle(&mut self, cmd: Cmd) {
        if self.remote_active {
            if let Some(r) = self.remote.clone() {
                match &cmd {
                    Cmd::Play => return r.play(),
                    Cmd::Pause => return r.pause(),
                    Cmd::Toggle => return r.play_pause(),
                    Cmd::Next => return r.next(),
                    Cmd::Prev => return r.prev(),
                    Cmd::Seek(ms) => return r.seek(*ms),
                    Cmd::SetVolume(v) => r.set_volume(*v),
                    Cmd::PlayContext { .. } | Cmd::PlayTrack(_) | Cmd::ExtendContext(_) => self.leave_remote(),
                    _ => {}
                }
            }
        }
        match cmd {
            Cmd::AttachSession(_) | Cmd::CreateConnectPlayer(..) => {}
            Cmd::SetRemote(r) => self.remote = Some(r),
            Cmd::RemoteVolume(v) => {
                let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { self.prefs.volume };
                self.prefs.volume = v;
                self.audio.set_volume(v);
                self.state.volume = v;
                self.emit_state(false);
            }
            Cmd::PlayContext { info, tracks, start } => {
                let e = self.queue.play_context(info, tracks, start);
                self.start_entry(e, true);
            }
            Cmd::PlayTrack(t) => {
                let e = self.queue.play_now(t);
                self.start_entry(Some(e), true);
            }
            Cmd::Play => self.resume(),
            Cmd::Pause => {
                self.audio.pause(self.active);
            }
            Cmd::Toggle => {
                if self.state.playing {
                    self.audio.pause(self.active)
                } else {
                    self.resume()
                }
            }
            Cmd::Next => {
                let e = self.queue.next(false);
                self.after_advance(e, true);
            }
            Cmd::Prev => {
                if self.position_now() > 3000 || self.queue.history.is_empty() {
                    self.seek(0);
                } else {
                    let e = self.queue.prev();
                    self.start_entry(e, true);
                }
            }
            Cmd::Seek(ms) => self.seek(ms),
            Cmd::SetVolume(v) => {
                let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { self.prefs.volume };
                self.prefs.volume = v;
                self.audio.set_volume(v);
                self.state.volume = v;
                self.emit_state(false);
            }
            Cmd::SetShuffle(m) => {
                self.queue.set_shuffle(m);
                self.state.shuffle = m;
                self.preloaded_for = None;
                self.emit_state(false);
                self.queue_changed();
            }
            Cmd::SetRepeat(m) => {
                self.queue.set_repeat(m);
                self.state.repeat = m;
                self.preloaded_for = None;
                self.emit_state(false);
                self.queue_changed();
            }
            Cmd::AddToQueue(t) => {
                self.queue.add_to_queue(t);
                self.preloaded_for = None;
                self.queue_changed();
            }
            Cmd::PlayNext(t) => {
                self.queue.play_next(t);
                self.preloaded_for = None;
                self.queue_changed();
            }
            Cmd::RemoveFromQueue(uid) => {
                if self.queue.remove(uid) {
                    self.preloaded_for = None;
                    self.queue_changed();
                }
            }
            Cmd::MoveInQueue { uid, to } => {
                if self.queue.move_entry(uid, to) {
                    self.preloaded_for = None;
                    self.queue_changed();
                }
            }
            Cmd::ClearQueue => {
                self.queue.clear_user_queue();
                self.queue_changed();
            }
            Cmd::SetEq(eq) => {
                let eq = eq.sanitized();
                self.audio.set_eq(eq.clone());
                *self.shared_eq.write().unwrap() = eq.clone();
                self.prefs.eq = eq;
                self.dirty_queue = true;
            }
            Cmd::SetCrossfade(ms) => {
                self.prefs.crossfade_ms = ms.min(12_000);
                self.state.crossfade_ms = self.prefs.crossfade_ms;
                self.emit_state(false);
            }
            Cmd::ExtendContext(tracks) => {
                self.autoplay_pending = false;
                if tracks.is_empty() {
                    return;
                }
                self.queue.extend_context(tracks);
                self.queue_changed();
                if self.state.track.is_none() || (!self.state.playing && !self.state.loading) {
                    let e = self.queue.next(true);
                    self.start_entry(e, true);
                }
            }
        }
    }

    fn resume(&mut self) {
        // After a restart nothing is loaded yet: load the persisted current track.
        if self.decks[self.active].uri.is_none() {
            if let Some(cur) = self.queue.current.clone() {
                let pos = self.state.position_ms;
                self.load_on(self.active, &cur.track.uri, true, pos);
                return;
            }
        }
        self.audio.play(self.active);
    }

    fn seek(&mut self, ms: u32) {
        let ms = ms.min(self.state.duration_ms.max(1));
        if self.decks[self.active].uri.is_none() {
            self.state.position_ms = ms;
            return self.resume();
        }
        self.audio.seek(self.active, ms);
        self.pos_sample = (ms, Instant::now());
        self.state.position_ms = ms;
        self.state.position_at = unix_ms();
        self.emit_state(false);
    }

    fn load_on(&mut self, deck: usize, uri: &str, play: bool, pos: u32) {
        self.decks[deck].awaiting_id = true;
        self.decks[deck].request_id = None;
        self.decks[deck].uri = Some(uri.to_string());
        if let Err(e) = self.audio.load(deck, uri, play, pos) {
            warn!("load failed: {e}");
            let _ = self.events.send(Event::Error(e.to_string()));
        }
    }

    /// Start an entry immediately on the active deck (manual action: no crossfade).
    fn start_entry(&mut self, e: Option<Entry>, play: bool) {
        let Some(e) = e else {
            self.on_queue_exhausted();
            return;
        };
        if self.crossfading {
            // Abort a running crossfade: silence the other deck.
            let other = 1 - self.active;
            self.audio.stop(other);
            self.decks[other].uri = None;
            self.crossfading = false;
        }
        self.audio.set_deck_level(self.active, true);
        self.load_on(self.active, &e.track.uri, play, 0);
        self.set_current_track(&e.track);
        self.queue_changed();
    }

    fn after_advance(&mut self, e: Option<Entry>, manual: bool) {
        if manual {
            self.start_entry(e, true);
        } else {
            match e {
                Some(e) => {
                    // Gapless: the track was preloaded, librespot starts it from memory.
                    self.load_on(self.active, &e.track.uri, true, 0);
                    self.set_current_track(&e.track);
                    self.queue_changed();
                }
                None => self.on_queue_exhausted(),
            }
        }
    }

    fn on_queue_exhausted(&mut self) {
        self.state.playing = false;
        self.state.loading = false;
        self.emit_state(false);
        if !self.autoplay_pending {
            self.autoplay_pending = true;
            let seeds: Vec<String> = self
                .queue
                .history
                .iter()
                .rev()
                .take(5)
                .map(|e| e.track.uri.clone())
                .filter(|u| u.starts_with("spotify:track:"))
                .collect();
            let _ = self.events.send(Event::NeedAutoplay {
                seed_uris: seeds,
                context_uri: self.queue.context.as_ref().map(|c| c.uri.clone()),
            });
        }
    }

    fn set_current_track(&mut self, t: &Track) {
        self.state.track = Some(t.clone());
        self.state.duration_ms = t.duration_ms;
        self.state.position_ms = 0;
        self.state.position_at = unix_ms();
        self.state.loading = true;
        self.state.context = self.queue.context.clone();
        self.pos_sample = (0, Instant::now());
        self.preloaded_for = None;
        self.emit_state(true);
    }

    fn position_now(&self) -> u32 {
        if self.state.playing {
            self.pos_sample.0 + self.pos_sample.1.elapsed().as_millis() as u32
        } else {
            self.pos_sample.0
        }
    }

    fn on_player_event(&mut self, deck: usize, ev: PlayerEvent) {
        if let PlayerEvent::PlayRequestIdChanged { play_request_id } = ev {
            if self.decks[deck].awaiting_id {
                self.decks[deck].request_id = Some(play_request_id);
                self.decks[deck].awaiting_id = false;
            }
            return;
        }
        if let PlayerEvent::TrackChanged { audio_item } = &ev {
            if deck == self.active && audio_item.duration_ms > 0 {
                self.state.duration_ms = audio_item.duration_ms;
                if let Some(t) = &mut self.state.track {
                    if t.duration_ms == 0 {
                        t.duration_ms = audio_item.duration_ms;
                    }
                    if t.title.is_empty() {
                        t.title = audio_item.name.clone();
                    }
                }
                self.emit_state(false);
            }
            return;
        }
        let id = ev.get_play_request_id();
        if id.is_some() && id != self.decks[deck].request_id {
            return; // stale event from an earlier load
        }
        let is_active = deck == self.active;
        match ev {
            PlayerEvent::Loading { .. } if is_active => {
                self.state.loading = true;
                self.emit_state(false);
            }
            PlayerEvent::Playing { position_ms, .. } if is_active => {
                self.state.playing = true;
                self.state.loading = false;
                self.pos_sample = (position_ms, Instant::now());
                self.state.position_ms = position_ms;
                self.state.position_at = unix_ms();
                self.emit_state(false);
            }
            PlayerEvent::Paused { position_ms, .. } if is_active => {
                self.state.playing = false;
                self.state.loading = false;
                self.pos_sample = (position_ms, Instant::now());
                self.state.position_ms = position_ms;
                self.state.position_at = unix_ms();
                self.emit_state(false);
            }
            PlayerEvent::PositionChanged { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. }
                if is_active =>
            {
                self.pos_sample = (position_ms, Instant::now());
                self.state.position_ms = position_ms;
                self.state.position_at = unix_ms();
            }
            PlayerEvent::TimeToPreloadNextTrack { .. } if is_active && self.prefs.crossfade_ms == 0 => {
                self.preload_next(self.active);
            }
            PlayerEvent::EndOfTrack { .. } => {
                if !is_active {
                    // The deck we faded out finished; nothing to do.
                    self.decks[deck].uri = None;
                    return;
                }
                if self.crossfading {
                    return;
                }
                let e = self.queue.next(true);
                self.after_advance(e, false);
            }
            PlayerEvent::Unavailable { track_id, .. } if is_active => {
                warn!("unavailable: {track_id:?}");
                let _ = self.events.send(Event::Error("Track unavailable — skipping".into()));
                let e = self.queue.next(false);
                self.after_advance(e, false);
            }
            PlayerEvent::Stopped { .. } if is_active => {
                self.state.playing = false;
                self.emit_state(false);
            }
            _ => {}
        }
    }

    fn preload_next(&mut self, deck: usize) {
        let cur_uid = self.queue.current.as_ref().map(|e| e.uid);
        if self.preloaded_for == cur_uid {
            return;
        }
        if let Some(t) = self.queue.peek_next() {
            debug!("preloading {}", t.uri);
            self.audio.preload(deck, &t.uri);
            self.preloaded_for = cur_uid;
        }
    }

    fn on_tick(&mut self) {
        if !self.state.playing {
            return;
        }
        let pos = self.position_now();
        let _ = self.events.send(Event::Progress { position_ms: pos, duration_ms: self.state.duration_ms });
        let xf = self.prefs.crossfade_ms;
        let dur = self.state.duration_ms;
        if xf == 0 || dur == 0 || self.crossfading || self.queue.repeat == RepeatMode::One {
            return;
        }
        // Warm up the other deck a little before the fade starts.
        if pos + xf + 8000 >= dur {
            self.preload_next(1 - self.active);
        }
        if pos + xf >= dur && dur > xf * 2 {
            let Some(e) = self.queue.next(true) else { return };
            let old = self.active;
            let new = 1 - old;
            info!("crossfade {} ms -> {}", xf, e.track.uri);
            self.audio.set_deck_level(new, false);
            self.load_on(new, &e.track.uri, true, 0);
            self.audio.fade(new, true, xf);
            self.audio.fade(old, false, xf);
            self.active = new;
            self.crossfading = true;
            self.set_current_track(&e.track);
            self.queue_changed();
            // Stop the old deck once the fade has finished.
            self.pending_stop = Some((old, Instant::now() + Duration::from_millis(xf as u64 + 200)));
        }
        if let Some((deck, at)) = self.pending_stop {
            if Instant::now() >= at {
                self.audio.stop(deck);
                self.decks[deck].uri = None;
                self.pending_stop = None;
                self.crossfading = false;
            }
        }
    }

    fn emit_state(&mut self, track_changed: bool) {
        *self.shared_state.write().unwrap() = self.state.clone();
        let ev = if track_changed {
            Event::TrackChanged(self.state.clone())
        } else {
            Event::PlayStateChanged(self.state.clone())
        };
        let _ = self.events.send(ev);
    }

    fn queue_changed(&mut self) {
        let v = self.queue.view(50);
        *self.shared_queue.write().unwrap() = v.clone();
        let _ = self.events.send(Event::QueueChanged(v));
        self.dirty_queue = true;
    }

    fn leave_remote(&mut self) {
        if let Some(r) = &self.remote {
            r.pause();
            r.release();
        }
        self.remote_active = false;
        self.state.remote = false;
        self.audio.set_deck_level(engine_audio::CONNECT_DECK, false);
    }

    /// Events from the Spotify Connect deck.
    fn on_connect_event(&mut self, ev: PlayerEvent) {
        match ev {
            PlayerEvent::PlayRequestIdChanged { play_request_id } => self.remote_req = Some(play_request_id),
            PlayerEvent::TrackChanged { audio_item } => {
                let t = track_from_audio_item(&audio_item);
                if self.remote_active {
                    self.state.track = Some(t);
                    self.state.duration_ms = audio_item.duration_ms;
                    self.state.position_ms = 0;
                    self.state.position_at = unix_ms();
                    self.emit_state(true);
                } else {
                    self.remote_track = Some((t, audio_item.duration_ms));
                }
            }
            PlayerEvent::Playing { position_ms, .. } => {
                if !self.remote_active {
                    // The phone started playing on us: pause local, mirror remote.
                    info!("Spotify Connect took over playback");
                    if self.state.playing {
                        self.audio.pause(self.active);
                    }
                    self.remote_active = true;
                    self.state.remote = true;
                    self.audio.set_deck_level(engine_audio::CONNECT_DECK, true);
                    if let Some((t, d)) = self.remote_track.take() {
                        self.state.track = Some(t);
                        self.state.duration_ms = d;
                    }
                    self.state.playing = true;
                    self.state.loading = false;
                    self.state.position_ms = position_ms;
                    self.state.position_at = unix_ms();
                    self.pos_sample = (position_ms, Instant::now());
                    self.emit_state(true);
                    return;
                }
                self.state.playing = true;
                self.state.loading = false;
                self.state.position_ms = position_ms;
                self.state.position_at = unix_ms();
                self.pos_sample = (position_ms, Instant::now());
                self.emit_state(false);
            }
            PlayerEvent::Paused { .. } | PlayerEvent::Stopped { .. } if self.remote_active => {
                let pos = if let PlayerEvent::Paused { position_ms, .. } = ev { position_ms } else { self.state.position_ms };
                self.state.playing = false;
                self.state.position_ms = pos;
                self.state.position_at = unix_ms();
                self.pos_sample = (pos, Instant::now());
                self.emit_state(false);
            }
            PlayerEvent::PositionChanged { position_ms, .. } | PlayerEvent::Seeked { position_ms, .. } | PlayerEvent::PositionCorrection { position_ms, .. }
                if self.remote_active =>
            {
                self.state.position_ms = position_ms;
                self.state.position_at = unix_ms();
                self.pos_sample = (position_ms, Instant::now());
            }
            PlayerEvent::Loading { .. } if self.remote_active => {
                self.state.loading = true;
                self.emit_state(false);
            }
            _ => {}
        }
    }

    fn persist(&mut self) {
        if !self.dirty_queue {
            return;
        }
        self.dirty_queue = false;
        if let Err(e) = self.store.save(QUEUE_KEY, &self.queue) {
            warn!("persist queue: {e}");
        }
        let _ = self.store.save(PREFS_KEY, &self.prefs);
    }
}

async fn recv_opt(ch: &mut Option<engine_audio::PlayerEventChannel>) -> Option<PlayerEvent> {
    match ch {
        Some(c) => c.recv().await,
        None => std::future::pending().await,
    }
}

/// Spawn with the persisted crossfade setting applied.
pub fn spawn_with(audio: AudioEngine, store: Arc<StateStore>, crossfade_ms: u32) -> PlaybackHandle {
    let h = spawn(audio, store);
    h.send(Cmd::SetCrossfade(crossfade_ms));
    h
}

/// Map a librespot AudioItem (from the Connect deck) to our Track model.
fn track_from_audio_item(a: &engine_audio::librespot_playback_metadata::AudioItem) -> Track {
    use engine_api::models::{AlbumRef, ArtistRef, Image};
    use engine_audio::librespot_playback_metadata::UniqueFields;
    let images: Vec<Image> = a
        .covers
        .iter()
        .filter_map(|c| {
            Some(Image {
                url: engine_api::parse::img_url(&c.url)?,
                width: Some(c.width.max(0) as u32),
                height: Some(c.height.max(0) as u32),
            })
        })
        .collect();
    let (artists, album) = match &a.unique_fields {
        UniqueFields::Track { artists, album, .. } => (
            artists.iter().map(|x| ArtistRef { uri: x.id.to_uri().unwrap_or_default(), name: x.name.clone() }).collect(),
            album.clone(),
        ),
        UniqueFields::Episode { show_name, .. } => (vec![ArtistRef { uri: String::new(), name: show_name.clone() }], show_name.clone()),
        _ => (Vec::new(), String::new()),
    };
    Track {
        uri: a.uri.clone(),
        title: a.name.clone(),
        artists,
        album: AlbumRef { uri: String::new(), name: album, images },
        duration_ms: a.duration_ms,
        explicit: a.is_explicit,
        playable: true,
        ..Default::default()
    }
}
