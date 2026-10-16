//! Lyrics: Spotify's color-lyrics (internal endpoint) first, then LRCLIB, then
//! Genius (plain text, no timestamps), local romanization, optional online
//! translation (off by default).

use anyhow::Result;
use engine_api::{keys, models::*};
use serde_json::Value;

use crate::app::State;

pub async fn get(state: &State, uri: &str) -> Result<Option<Value>> {
    let settings = state.settings();
    // ":g" = looked up with the Genius fallback too (older cached misses are retried).
    let key = format!("{}:g", keys::lyrics(uri));
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
            if l.is_none() && settings.lyrics.genius_fallback {
                l = genius(state, uri).await.unwrap_or_else(|e| {
                    log::debug!("genius: {e:#}");
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
        .user_agent(concat!("openmp3/", env!("CARGO_PKG_VERSION"), " (personal player)"))
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

// ------------------------------------------------------------------ Genius
//
// Genius has no public lyrics API: its search endpoint finds the song page and the
// lyrics are read from the page's `data-lyrics-container` blocks.

const GENIUS_UA: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36 openmp3/",
    env!("CARGO_PKG_VERSION")
);

async fn genius(state: &State, uri: &str) -> Result<Option<Lyrics>> {
    let map = state.api.tracks(&[uri.to_string()]).await?;
    let Some(t) = map.get(uri) else { return Ok(None) };
    let artist = t.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    let client = reqwest::Client::builder().user_agent(GENIUS_UA).timeout(std::time::Duration::from_secs(8)).build()?;
    let q = format!("{} {}", artist, strip_title(&t.title));
    let search: Value = client
        .get("https://genius.com/api/search/song")
        .query(&[("q", q.as_str()), ("per_page", "10")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let hits: Vec<Value> = search["response"]["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["hits"].as_array().cloned().unwrap_or_default())
        .map(|h| h["result"].clone())
        .collect();
    let Some(url) = best_genius_hit(&hits, &t.title, &artist) else { return Ok(None) };
    if !url.starts_with("https://genius.com/") {
        return Ok(None);
    }
    let html = client.get(&url).send().await?.error_for_status()?.text().await?;
    let Some(text) = extract_genius_lyrics(&html) else { return Ok(None) };
    let lines: Vec<LyricsLine> = text.lines().map(|l| LyricsLine { start_ms: 0, text: l.to_string() }).collect();
    Ok((!lines.is_empty()).then(|| Lyrics { synced: false, source: "Genius".into(), lines }))
}

/// Title without "(feat. …)", "[Remix]" or " - Remastered 2011" style suffixes.
fn strip_title(title: &str) -> String {
    let mut s = String::new();
    let mut depth = 0i32;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => s.push(c),
            _ => {}
        }
    }
    let s = s.split(" - ").next().unwrap_or(&s).to_string();
    s.trim().to_string()
}

fn norm(s: &str) -> String {
    let s = strip_title(s).to_lowercase().replace('&', "and");
    let s: String = s.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The search result that is this song by this artist (not a translation, cover or
/// a different song with a similar name). Returns its page URL.
fn best_genius_hit(hits: &[Value], title: &str, artist: &str) -> Option<String> {
    let (t, a) = (norm(title), norm(artist));
    if t.is_empty() || a.is_empty() {
        return None;
    }
    let mut best: Option<(i32, String)> = None;
    for h in hits {
        let ht = norm(h["title"].as_str().unwrap_or_default());
        let ha = norm(h["primary_artist"]["name"].as_str().unwrap_or_default());
        let all = norm(h["artist_names"].as_str().unwrap_or_default());
        let artist_ok = ha == a || ha.contains(&a) || a.contains(&ha) && !ha.is_empty() || all.contains(&a);
        let title_ok = ht == t || (ht.contains(&t) || t.contains(&ht)) && !ht.is_empty();
        if !artist_ok || !title_ok {
            continue;
        }
        let mut score = if ht == t { 10 } else { 5 } + if ha == a { 5 } else { 2 };
        if h["lyrics_state"].as_str() == Some("complete") {
            score += 3;
        }
        if let Some(url) = h["url"].as_str() {
            if best.as_ref().map(|(b, _)| score > *b).unwrap_or(true) {
                best = Some((score, url.to_string()));
            }
        }
    }
    best.map(|(_, u)| u)
}

/// Lyrics text from a Genius song page: the `data-lyrics-container` blocks, with
/// `<br>` as line breaks, other tags dropped, and the parts Genius marks
/// `data-exclude-from-selection` (header, ads) skipped. Section labels such as
/// "[Chorus]" are removed so it reads like the other lyrics sources.
pub fn extract_genius_lyrics(html: &str) -> Option<String> {
    const MARK: &str = "data-lyrics-container=\"true\"";
    let mut raw = String::new();
    let mut pos = 0;
    while let Some(i) = html[pos..].find(MARK) {
        let start = pos + i;
        let Some(gt) = html[start..].find('>') else { break };
        let mut j = start + gt + 1;
        let mut depth = 1usize; // open <div>s, counting the container itself
        let mut skip_at: Option<usize> = None;
        while depth > 0 {
            let Some(lt) = html[j..].find('<') else { j = html.len(); break };
            if skip_at.is_none() {
                raw.push_str(&decode_entities(&html[j..j + lt]));
            }
            let tag_start = j + lt;
            let Some(tag_len) = html[tag_start..].find('>') else { j = html.len(); break };
            let tag = &html[tag_start..tag_start + tag_len + 1];
            j = tag_start + tag_len + 1;
            if tag.starts_with("</div") {
                depth -= 1;
                if skip_at == Some(depth) {
                    skip_at = None;
                }
            } else if tag.starts_with("<div") && !tag.ends_with("/>") {
                if skip_at.is_none() && tag.contains("data-exclude-from-selection=\"true\"") {
                    skip_at = Some(depth);
                }
                depth += 1;
            } else if tag.starts_with("<br") && skip_at.is_none() {
                raw.push('\n');
            }
        }
        raw.push('\n');
        pos = j;
    }
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        if line.is_empty() && out.last().map(|l| l.is_empty()).unwrap_or(true) {
            continue; // no leading or doubled blank lines
        }
        out.push(line.to_string());
    }
    while out.last().map(|l| l.is_empty()).unwrap_or(false) {
        out.pop();
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if name.starts_with("#x") || name.starts_with("#X") => u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32),
                _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_lyrics_and_skips_header_and_ads() {
        let html = r#"<div class="Lyrics__Root"><div data-lyrics-container="true" class="x"><div data-exclude-from-selection="true" class="LyricsHeader"><button><span>163 Contributors</span></button><div>Karma Police Lyrics</div></div>[Verse 1]<br/>Karma police, arrest this man<br/>He talks in <a href="/1"><span>maths</span></a><br/><br/>[Chorus]<br/>This is what you&#x27;ll get &amp; more</div><div data-exclude-from-selection="true" class="ad"><div id="gpt"></div></div><div data-lyrics-container="true" class="x">[Outro]<br/>For a minute there<br/>I lost myself</div></div>"#;
        assert_eq!(
            extract_genius_lyrics(html).unwrap(),
            "Karma police, arrest this man\nHe talks in maths\n\nThis is what you'll get & more\nFor a minute there\nI lost myself"
        );
        assert_eq!(extract_genius_lyrics("<html>no lyrics here</html>"), None);
    }

    #[test]
    fn picks_the_right_song_not_a_translation() {
        let hits = vec![
            json!({"title": "Radiohead - Karma Police (Türkçe Çeviri)", "primary_artist": {"name": "Genius Türkçe Çeviriler"}, "artist_names": "Genius Türkçe Çeviriler", "url": "https://genius.com/tr", "lyrics_state": "complete"}),
            json!({"title": "Karma Police", "primary_artist": {"name": "Radiohead"}, "artist_names": "Radiohead", "url": "https://genius.com/Radiohead-karma-police-lyrics", "lyrics_state": "complete"}),
        ];
        assert_eq!(best_genius_hit(&hits, "Karma Police - Remastered", "Radiohead").as_deref(), Some("https://genius.com/Radiohead-karma-police-lyrics"));
        assert_eq!(best_genius_hit(&hits, "Paranoid Android", "Radiohead"), None);
        assert_eq!(strip_title("Song (feat. Someone) - Remastered 2011"), "Song");
    }

    /// Live check against genius.com: `cargo test genius_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn genius_live() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::builder().user_agent(GENIUS_UA).build().unwrap();
            for (artist, title) in [("Radiohead", "Karma Police"), ("Kendrick Lamar", "HUMBLE."), ("Daft Punk", "Get Lucky (feat. Pharrell Williams)")] {
                let q = format!("{artist} {}", strip_title(title));
                let v: Value = client.get("https://genius.com/api/search/song").query(&[("q", q.as_str()), ("per_page", "10")]).send().await.unwrap().json().await.unwrap();
                let hits: Vec<Value> = v["response"]["sections"].as_array().into_iter().flatten().flat_map(|s| s["hits"].as_array().cloned().unwrap_or_default()).map(|h| h["result"].clone()).collect();
                let url = best_genius_hit(&hits, title, artist).expect("hit");
                let html = client.get(&url).send().await.unwrap().text().await.unwrap();
                let text = extract_genius_lyrics(&html).expect("lyrics");
                let lines: Vec<&str> = text.lines().collect();
                println!("{artist} - {title}: {url} -> {} lines; first: {:?}", lines.len(), lines.first());
                if std::env::var("GENIUS_DUMP").is_ok() { println!("<<<
{text}
>>>"); }
                assert!(lines.len() > 8);
            }
        });
    }
}

