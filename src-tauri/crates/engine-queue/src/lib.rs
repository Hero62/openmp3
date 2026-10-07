//! Persistent play queue.
//!
//! Spotify-style model with two parts:
//! * the **user queue** ("Play next" / "Add to queue") — always plays first,
//!   survives restarts and is *not* wiped when you start playing something else;
//! * the **context** (playlist / album / artist / radio) with its play order,
//!   which is a shuffle bag when shuffle is on.
//!
//! The queue only decides *what* plays next; the engine performs playback.

pub mod shuffle;
pub mod store;

use std::collections::VecDeque;

use engine_api::models::Track;
use serde::{Deserialize, Serialize};
use shuffle::Rng;

pub const HISTORY_CAP: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShuffleMode {
    #[default]
    Off,
    On,
    /// Shuffle that spreads out tracks by the same artist.
    Spread,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Unique per queue entry (the same track can be queued twice).
    pub uid: u64,
    pub track: Track,
    /// True for items added with Play next / Add to queue.
    pub from_user_queue: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub uri: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub context: Option<ContextInfo>,
    context_tracks: Vec<Track>,
    /// Play order: indices into `context_tracks`.
    order: Vec<usize>,
    /// Position in `order` of the current context track (None before start).
    pos: Option<usize>,
    pub user_queue: VecDeque<Entry>,
    pub current: Option<Entry>,
    pub history: Vec<Entry>,
    pub shuffle: ShuffleMode,
    pub repeat: RepeatMode,
    next_uid: u64,
    #[serde(skip, default = "Rng::new")]
    rng: Rng,
}

impl Default for Rng {
    fn default() -> Self {
        Rng::new()
    }
}

impl std::fmt::Debug for Rng {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rng")
    }
}

impl Clone for Rng {
    fn clone(&self) -> Self {
        Rng::new()
    }
}

/// What the UI shows in the queue panel.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueView {
    pub current: Option<Entry>,
    pub user_queue: Vec<Entry>,
    pub up_next: Vec<Entry>,
    pub context: Option<ContextInfo>,
    pub shuffle: ShuffleMode,
    pub repeat: RepeatMode,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn with_seed(seed: u64) -> Self {
        Self { rng: Rng::seeded(seed), ..Self::default() }
    }

    fn entry(&mut self, track: Track, from_user_queue: bool) -> Entry {
        self.next_uid += 1;
        Entry { uid: self.next_uid, track, from_user_queue }
    }

    fn build_order(&mut self, first: Option<usize>) {
        let n = self.context_tracks.len();
        self.order = match self.shuffle {
            ShuffleMode::Off => (0..n).collect(),
            ShuffleMode::On => shuffle::bag(n, first, &mut self.rng),
            ShuffleMode::Spread => {
                let artists: Vec<String> = self
                    .context_tracks
                    .iter()
                    .map(|t| t.artists.first().map(|a| a.uri.clone()).unwrap_or_default())
                    .collect();
                shuffle::spread(&artists, first, &mut self.rng)
            }
        };
    }

    /// Start playing a context at `start` (index into `tracks`). The user
    /// queue is kept. Returns the entry to play now.
    pub fn play_context(&mut self, info: ContextInfo, tracks: Vec<Track>, start: Option<usize>) -> Option<Entry> {
        self.push_current_to_history();
        self.context = Some(info);
        self.context_tracks = tracks;
        let start = start.filter(|&s| s < self.context_tracks.len());
        let first = start.or(if self.shuffle == ShuffleMode::Off { Some(0) } else { None });
        self.build_order(first);
        if self.order.is_empty() {
            self.pos = None;
            self.current = None;
            return None;
        }
        self.pos = Some(match start {
            Some(s) => self.order.iter().position(|&i| i == s).unwrap_or(0),
            None => 0,
        });
        let t = self.context_tracks[self.order[self.pos.unwrap()]].clone();
        let e = self.entry(t, false);
        self.current = Some(e.clone());
        Some(e)
    }

    /// Play a single track right now without touching the context or queue.
    pub fn play_now(&mut self, track: Track) -> Entry {
        self.push_current_to_history();
        let e = self.entry(track, true);
        self.current = Some(e.clone());
        e
    }

    pub fn add_to_queue(&mut self, track: Track) -> u64 {
        let e = self.entry(track, true);
        let uid = e.uid;
        self.user_queue.push_back(e);
        uid
    }

    pub fn play_next(&mut self, track: Track) -> u64 {
        let e = self.entry(track, true);
        let uid = e.uid;
        self.user_queue.push_front(e);
        uid
    }

    pub fn remove(&mut self, uid: u64) -> bool {
        let before = self.user_queue.len();
        self.user_queue.retain(|e| e.uid != uid);
        before != self.user_queue.len()
    }

    /// Move a user-queue entry to index `to` (clamped).
    pub fn move_entry(&mut self, uid: u64, to: usize) -> bool {
        let Some(from) = self.user_queue.iter().position(|e| e.uid == uid) else { return false };
        let e = self.user_queue.remove(from).unwrap();
        let to = to.min(self.user_queue.len());
        self.user_queue.insert(to, e);
        true
    }

    pub fn clear_user_queue(&mut self) {
        self.user_queue.clear();
    }

    fn push_current_to_history(&mut self) {
        if let Some(c) = self.current.take() {
            self.history.push(c);
            if self.history.len() > HISTORY_CAP {
                let drop = self.history.len() - HISTORY_CAP;
                self.history.drain(..drop);
            }
        }
    }

    /// Advance. `auto` is true when the previous track ended by itself
    /// (repeat-one only applies then). Returns None when everything is done
    /// (the engine then asks autoplay for more).
    pub fn next(&mut self, auto: bool) -> Option<Entry> {
        if auto && self.repeat == RepeatMode::One {
            if let Some(c) = &self.current {
                return Some(c.clone());
            }
        }
        if let Some(e) = self.user_queue.pop_front() {
            self.push_current_to_history();
            self.current = Some(e.clone());
            return Some(e);
        }
        if self.order.is_empty() {
            self.push_current_to_history();
            return None;
        }
        let next_pos = match self.pos {
            None => 0,
            Some(p) if p + 1 < self.order.len() => p + 1,
            Some(_) => {
                if self.repeat == RepeatMode::Off {
                    self.push_current_to_history();
                    self.pos = Some(self.order.len() - 1);
                    return None;
                }
                // Repeat all: new bag, avoiding an immediate repeat of the last track.
                let last = self.order.last().copied();
                if self.shuffle != ShuffleMode::Off {
                    self.build_order(None);
                    if self.order.len() > 1 && self.order.first().copied() == last {
                        let n = self.order.len();
                        self.order.swap(0, 1 + self.rng.below(n - 1));
                    }
                }
                0
            }
        };
        self.push_current_to_history();
        self.pos = Some(next_pos);
        let t = self.context_tracks[self.order[next_pos]].clone();
        let e = self.entry(t, false);
        self.current = Some(e.clone());
        Some(e)
    }

    /// Go back to the previous track from history. (Restart-if-past-3s is
    /// decided by the engine before calling this.)
    pub fn prev(&mut self) -> Option<Entry> {
        let prev = self.history.pop()?;
        if let Some(cur) = self.current.take() {
            if cur.from_user_queue {
                self.user_queue.push_front(cur);
            } else if let Some(p) = self.pos {
                // Step the context back so `next` returns this track again.
                self.pos = p.checked_sub(1);
            }
        }
        if !prev.from_user_queue {
            if let Some(i) = self.context_tracks.iter().position(|t| t.uri == prev.track.uri) {
                if let Some(p) = self.order.iter().position(|&o| o == i) {
                    self.pos = Some(p);
                }
            }
        }
        self.current = Some(prev.clone());
        Some(prev)
    }

    pub fn set_shuffle(&mut self, mode: ShuffleMode) {
        if self.shuffle == mode {
            return;
        }
        self.shuffle = mode;
        let cur_idx = self.pos.and_then(|p| self.order.get(p).copied());
        self.build_order(cur_idx);
        self.pos = cur_idx.and_then(|i| self.order.iter().position(|&o| o == i));
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// The next `n` context tracks after the current one (for the UI / preload).
    pub fn upcoming(&self, n: usize) -> Vec<Track> {
        let start = self.pos.map(|p| p + 1).unwrap_or(0);
        self.order.iter().skip(start).take(n).map(|&i| self.context_tracks[i].clone()).collect()
    }

    /// What will `next(true)` return, without mutating (for gapless preload).
    pub fn peek_next(&self) -> Option<Track> {
        if self.repeat == RepeatMode::One {
            return self.current.as_ref().map(|c| c.track.clone());
        }
        if let Some(e) = self.user_queue.front() {
            return Some(e.track.clone());
        }
        let p = self.pos.map(|p| p + 1).unwrap_or(0);
        if let Some(&i) = self.order.get(p) {
            return Some(self.context_tracks[i].clone());
        }
        if self.repeat == RepeatMode::All && self.shuffle == ShuffleMode::Off {
            return self.order.first().map(|&i| self.context_tracks[i].clone());
        }
        None
    }

    pub fn context_tracks(&self) -> &[Track] {
        &self.context_tracks
    }

    /// Append tracks to the context (autoplay / radio continuation).
    pub fn extend_context(&mut self, tracks: Vec<Track>) {
        let base = self.context_tracks.len();
        let n = tracks.len();
        self.context_tracks.extend(tracks);
        let mut new: Vec<usize> = (base..base + n).collect();
        if self.shuffle != ShuffleMode::Off {
            let perm = shuffle::bag(n, None, &mut self.rng);
            new = perm.into_iter().map(|i| base + i).collect();
        }
        self.order.extend(new);
    }

    pub fn view(&self, up_next: usize) -> QueueView {
        let mut uid = u64::MAX / 2;
        let up = self
            .upcoming(up_next)
            .into_iter()
            .map(|track| {
                uid += 1;
                Entry { uid, track, from_user_queue: false }
            })
            .collect();
        QueueView {
            current: self.current.clone(),
            user_queue: self.user_queue.iter().cloned().collect(),
            up_next: up,
            context: self.context.clone(),
            shuffle: self.shuffle,
            repeat: self.repeat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_api::models::ArtistRef;

    fn t(n: usize, artist: &str) -> Track {
        Track {
            uri: format!("spotify:track:{n}"),
            title: format!("T{n}"),
            artists: vec![ArtistRef { uri: artist.into(), name: artist.into() }],
            ..Default::default()
        }
    }
    fn tracks(n: usize) -> Vec<Track> {
        (0..n).map(|i| t(i, &format!("a{}", i % 3))).collect()
    }
    fn ctx() -> ContextInfo {
        ContextInfo { uri: "spotify:playlist:x".into(), name: "x".into() }
    }
    fn uri(e: &Option<Entry>) -> String {
        e.as_ref().map(|e| e.track.uri.clone()).unwrap_or_default()
    }

    #[test]
    fn linear_context_plays_in_order_and_stops() {
        let mut q = Queue::with_seed(1);
        assert_eq!(uri(&q.play_context(ctx(), tracks(3), Some(0))), "spotify:track:0");
        assert_eq!(uri(&q.next(true)), "spotify:track:1");
        assert_eq!(uri(&q.next(true)), "spotify:track:2");
        assert!(q.next(true).is_none());
    }

    #[test]
    fn user_queue_plays_first_and_survives_new_context() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(3), Some(0));
        q.add_to_queue(t(100, "z"));
        q.play_next(t(101, "z"));
        assert_eq!(uri(&q.next(false)), "spotify:track:101");
        // Start a different context: queue must not be wiped.
        q.play_context(ctx(), tracks(5), Some(2));
        assert_eq!(q.user_queue.len(), 1);
        assert_eq!(uri(&q.next(false)), "spotify:track:100");
        assert_eq!(uri(&q.next(false)), "spotify:track:3");
    }

    #[test]
    fn move_and_remove() {
        let mut q = Queue::with_seed(1);
        let a = q.add_to_queue(t(1, "x"));
        let b = q.add_to_queue(t(2, "x"));
        let c = q.add_to_queue(t(3, "x"));
        assert!(q.move_entry(c, 0));
        let order: Vec<u64> = q.user_queue.iter().map(|e| e.uid).collect();
        assert_eq!(order, vec![c, a, b]);
        assert!(q.remove(a));
        assert!(!q.remove(a));
        assert_eq!(q.user_queue.len(), 2);
    }

    #[test]
    fn shuffle_plays_every_track_once_before_repeating() {
        let mut q = Queue::with_seed(9);
        q.set_shuffle(ShuffleMode::On);
        q.set_repeat(RepeatMode::All);
        let n = 25;
        let mut seen = std::collections::HashSet::new();
        seen.insert(uri(&q.play_context(ctx(), tracks(n), Some(4))));
        assert!(seen.contains("spotify:track:4"));
        for _ in 1..n {
            assert!(seen.insert(uri(&q.next(true))), "repeat before bag exhausted");
        }
        assert_eq!(seen.len(), n);
        // Next round starts a fresh bag (no immediate repeat of the last track).
        let last = q.current.clone();
        let first_of_round = q.next(true);
        assert!(first_of_round.is_some());
        assert_ne!(uri(&first_of_round), uri(&last));
    }

    #[test]
    fn repeat_one_only_on_auto_advance() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(3), Some(0));
        q.set_repeat(RepeatMode::One);
        assert_eq!(uri(&q.next(true)), "spotify:track:0");
        assert_eq!(uri(&q.next(false)), "spotify:track:1");
    }

    #[test]
    fn prev_walks_history() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(4), Some(0));
        q.next(true);
        q.next(true);
        assert_eq!(uri(&q.prev()), "spotify:track:1");
        assert_eq!(uri(&q.next(false)), "spotify:track:2");
    }

    #[test]
    fn toggling_shuffle_keeps_current_track() {
        let mut q = Queue::with_seed(3);
        q.play_context(ctx(), tracks(10), Some(5));
        q.set_shuffle(ShuffleMode::Spread);
        assert_eq!(uri(&q.current), "spotify:track:5");
        assert_eq!(q.upcoming(100).len(), 9);
        q.set_shuffle(ShuffleMode::Off);
        assert_eq!(q.peek_next().unwrap().uri, "spotify:track:6");
    }

    #[test]
    fn autoplay_extension_continues_after_end() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(2), Some(0));
        q.next(true);
        assert!(q.next(true).is_none());
        q.extend_context(vec![t(50, "r"), t(51, "r")]);
        assert_eq!(uri(&q.next(true)), "spotify:track:50");
        assert_eq!(uri(&q.next(true)), "spotify:track:51");
    }

    #[test]
    fn serde_roundtrip() {
        let mut q = Queue::with_seed(1);
        q.play_context(ctx(), tracks(3), Some(1));
        q.add_to_queue(t(9, "z"));
        let json = serde_json::to_string(&q).unwrap();
        let mut back: Queue = serde_json::from_str(&json).unwrap();
        assert_eq!(uri(&back.current), "spotify:track:1");
        assert_eq!(uri(&back.next(false)), "spotify:track:9");
        assert_eq!(uri(&back.next(false)), "spotify:track:2");
    }
}
