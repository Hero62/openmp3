//! Persists the queue (and other small engine state) in `state.sqlite`.

use std::{path::Path, sync::Mutex};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

pub struct StateStore {
    conn: Mutex<Connection>,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn save<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
        conn.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, json],
        )?;
        Ok(())
    }

    pub fn load<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let conn = self.conn.lock().ok()?;
        let s: Option<String> =
            conn.query_row("SELECT value FROM state WHERE key = ?1", params![key], |r| r.get(0)).optional().ok()?;
        serde_json::from_str(&s?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContextInfo, Queue};

    #[test]
    fn queue_survives_reopen() {
        let dir = std::env::temp_dir().join(format!("openmp3-test-{}", std::process::id()));
        let path = dir.join("state.sqlite");
        {
            let s = StateStore::open(&path).unwrap();
            let mut q = Queue::new();
            q.play_context(ContextInfo { uri: "c".into(), name: "c".into() }, vec![Default::default()], Some(0));
            q.add_to_queue(engine_api::models::Track { uri: "spotify:track:q".into(), ..Default::default() });
            s.save("queue", &q).unwrap();
        }
        let s = StateStore::open(&path).unwrap();
        let q: Queue = s.load("queue").unwrap();
        assert_eq!(q.user_queue.len(), 1);
        assert_eq!(q.user_queue[0].track.uri, "spotify:track:q");
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
