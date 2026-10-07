//! SQLite key/value cache for API results (stale-while-revalidate).
//!
//! Every API read goes: return cached value instantly if present, refresh in
//! the background when older than its TTL, notify listeners if it changed.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

pub struct Cache {
    conn: Mutex<Connection>,
}

pub struct Cached<T> {
    pub value: T,
    pub age_secs: i64,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS kv (
                key TEXT PRIMARY KEY,
                value BLOB NOT NULL,
                fetched_at INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<Cached<T>> {
        let conn = self.conn.lock().ok()?;
        let row: Option<(Vec<u8>, i64)> = conn
            .query_row("SELECT value, fetched_at FROM kv WHERE key = ?1", params![key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .ok()?;
        let (blob, at) = row?;
        let value = serde_json::from_slice(&blob).ok()?;
        Some(Cached { value, age_secs: now() - at })
    }

    /// Store a value. Returns true if it differs from what was cached.
    pub fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<bool> {
        let blob = serde_json::to_vec(value)?;
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("cache poisoned"))?;
        let old: Option<Vec<u8>> = conn
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?;
        conn.execute(
            "INSERT INTO kv (key, value, fetched_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, fetched_at = excluded.fetched_at",
            params![key, blob, now()],
        )?;
        Ok(old.as_deref() != Some(blob.as_slice()))
    }

    pub fn invalidate(&self, key: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("DELETE FROM kv WHERE key = ?1", params![key]);
        }
    }

    pub fn invalidate_prefix(&self, prefix: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("DELETE FROM kv WHERE key LIKE ?1 || '%'", params![prefix]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_change_detection() {
        let c = Cache::in_memory().unwrap();
        assert!(c.get::<Vec<u32>>("a").is_none());
        assert!(c.put("a", &vec![1u32, 2]).unwrap());
        assert!(!c.put("a", &vec![1u32, 2]).unwrap());
        assert!(c.put("a", &vec![3u32]).unwrap());
        let got = c.get::<Vec<u32>>("a").unwrap();
        assert_eq!(got.value, vec![3]);
        assert!(got.age_secs <= 1);
        c.invalidate_prefix("a");
        assert!(c.get::<Vec<u32>>("a").is_none());
    }
}
