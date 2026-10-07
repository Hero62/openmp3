//! Lyrics: Spotify's color-lyrics (internal endpoint) first, LRCLIB as a
//! fallback, local romanization, optional online translation (off by default).

use anyhow::Result;
use engine_api::{keys, models::*};
use serde_json::Value;

use crate::app::State;

pub async fn get(state: &State, uri: &str) -> Result<Option<Value>> {
    let settings = state.settings();
    let key = keys::lyrics(uri);
    let lyrics: Option<Lyrics> = match state.api.cached::<Option<Lyrics>>(&key) {
        Some((l, _)) => l,
        None => {
            let mut l = state.api.spotify_lyrics(uri).await.unwrap_or_else(|e| {
                log::debug!("spotify lyrics: {e:#}");
                None
            });
            if l.is_none() && settings.lyrics.lrclib_fallback {
                l = lrclib(state, uri).await.unwrap_or_else(|e| {
                    log::debug!("lrclib: {e:#}");
                    None
                });
            }
            let _ = state.api.cache().put(&key, &l);
            l
        }
    };
    let Some(l) = lyrics else { return Ok(None) };
    let mut v = serde_json::to_value(&l)?;
    if settings.lyrics.romanize {
        if let Some(lines) = v["lines"].as_array_mut() {
            for line in lines {
                let text = line["text"].as_str().unwrap_or_default().to_string();
                if let Some(r) = crate::romanize::romanize(&text) {
                    line["roman"] = Value::String(r);
                }
            }
        }
    }
    if settings.lyrics.translate {
        if let Err(e) = crate::romanize::translate_lines(&mut v, &settings.lyrics.translate_to).await {
            log::warn!("translate: {e:#}");
        }
    }
    Ok(Some(v))
}

async fn lrclib(state: &State, uri: &str) -> Result<Option<Lyrics>> {
    let map = state.api.tracks(&[uri.to_string()]).await?;
    let Some(t) = map.get(uri) else { return Ok(None) };
    let artist = t.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("mp3palace/", env!("CARGO_PKG_VERSION"), " (personal player)"))
        .timeout(std::time::Duration::from_secs(8))
        .build()?;
    let resp = client
        .get("https://lrclib.net/api/get")
        .query(&[
            ("artist_name", artist.as_str()),
            ("track_name", t.title.as_str()),
            ("album_name", t.album.name.as_str()),
            ("duration", &(t.duration_ms / 1000).to_string()),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let v: Value = resp.json().await?;
    if let Some(synced) = v["syncedLyrics"].as_str().filter(|s| !s.is_empty()) {
        let lines = engine_api::parse::lrc(synced);
        if !lines.is_empty() {
            return Ok(Some(Lyrics { synced: true, source: "LRCLIB".into(), lines }));
        }
    }
    if let Some(plain) = v["plainLyrics"].as_str().filter(|s| !s.is_empty()) {
        let lines = plain.lines().map(|l| LyricsLine { start_ms: 0, text: l.to_string() }).collect();
        return Ok(Some(Lyrics { synced: false, source: "LRCLIB".into(), lines }));
    }
    Ok(None)
}
