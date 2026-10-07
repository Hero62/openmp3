//! `img://` (http://img.localhost) — local, cached proxy for Spotify cover art.
//!
//! Themes may only load images from this origin (their CSP forbids remote
//! URLs). Only Spotify CDN hosts are proxied:
//! * `/i/<hex>`        → https://i.scdn.co/image/<hex>
//! * `/x/<base64url>`  → any https URL on *.scdn.co / *.spotifycdn.com
//!
//! Also computes an accent color per cover for `accentFromCover` themes.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use tauri::http::{Request, Response};

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

const MAX_CACHE_BYTES: u64 = 300 * 1024 * 1024;

pub fn init(cache_dir: PathBuf) {
    let _ = std::fs::create_dir_all(&cache_dir);
    let _ = CACHE_DIR.set(cache_dir.clone());
    let _ = CLIENT.set(
        reqwest::Client::builder()
            .user_agent(concat!("openmp3/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("http client"),
    );
    std::thread::spawn(move || prune(&cache_dir, MAX_CACHE_BYTES));
}

fn prune(dir: &Path, cap: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.accessed().or_else(|_| m.modified()).ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, p) in files {
        if total <= cap * 8 / 10 {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Maps a local proxy path to (cache file name, remote URL). None = refused.
pub fn resolve(path: &str) -> Option<(String, String)> {
    if let Some(hex) = path.strip_prefix("/i/") {
        if !hex.is_empty() && hex.len() <= 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some((format!("i_{hex}"), format!("https://i.scdn.co/image/{hex}")));
        }
        return None;
    }
    if let Some(enc) = path.strip_prefix("/x/") {
        if enc.len() > 1024 {
            return None;
        }
        let url = String::from_utf8(b64url_decode(enc)?).ok()?;
        let rest = url.strip_prefix("https://")?;
        let host = rest.split('/').next()?;
        if host.contains('@') || host.contains(':') {
            return None;
        }
        if !(host.ends_with(".scdn.co") || host.ends_with(".spotifycdn.com")) {
            return None;
        }
        let name: String = enc.chars().filter(|c| c.is_ascii_alphanumeric()).take(120).collect();
        return Some((format!("x_{name}"), url));
    }
    None
}

pub async fn fetch(path: &str) -> Option<Arc<Vec<u8>>> {
    let (name, url) = resolve(path)?;
    let dir = CACHE_DIR.get()?;
    let file = dir.join(&name);
    if let Ok(b) = tokio::fs::read(&file).await {
        return Some(Arc::new(b));
    }
    let resp = CLIENT.get()?.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?.to_vec();
    if bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    let _ = tokio::fs::write(&file, &bytes).await;
    Some(Arc::new(bytes))
}

fn mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if b.starts_with(b"\x89PNG") {
        "image/png"
    } else if b.len() > 12 && &b[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

pub fn handle(request: Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn(async move {
        let resp = match fetch(&path).await {
            Some(b) => Response::builder()
                .header("Content-Type", mime(&b))
                .header("Cache-Control", "max-age=31536000, immutable")
                .header("Access-Control-Allow-Origin", "*")
                .header("X-Content-Type-Options", "nosniff")
                .body(b.to_vec())
                .unwrap(),
            None => Response::builder().status(404).body(Vec::new()).unwrap(),
        };
        responder.respond(resp);
    });
}

/// Accent color (#rrggbb) for a local proxy URL like `http://img.localhost/i/<hex>`.
/// Picks the most common vivid hue, then nudges lightness so it reads on dark UIs.
pub async fn accent(local_url: &str) -> Option<String> {
    let path = local_url.strip_prefix("http://img.localhost").or_else(|| local_url.strip_prefix("img://localhost"))?;
    let bytes = fetch(path).await?;
    tokio::task::spawn_blocking(move || accent_from_jpeg(&bytes)).await.ok()?
}

pub fn accent_from_jpeg(bytes: &[u8]) -> Option<String> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let px = dec.decode().ok()?;
    let (w, h) = dec.dimensions()?;
    if w == 0 || h == 0 || px.len() < w * h * 3 {
        return None;
    }
    // 24 hue buckets weighted by saturation * value.
    let mut buckets = [(0f64, 0f64, 0f64, 0f64); 24];
    let step = ((w * h) / 4096).max(1);
    let mut i = 0;
    while i < w * h {
        let (r, g, b) = (px[i * 3] as f64 / 255.0, px[i * 3 + 1] as f64 / 255.0, px[i * 3 + 2] as f64 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let sat = if max > 0.0 { d / max } else { 0.0 };
        if sat > 0.2 && max > 0.2 {
            let hue = if d == 0.0 {
                0.0
            } else if max == r {
                ((g - b) / d).rem_euclid(6.0)
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            } * 60.0;
            let k = ((hue / 15.0) as usize).min(23);
            let wgt = sat * max;
            buckets[k].0 += wgt;
            buckets[k].1 += r * wgt;
            buckets[k].2 += g * wgt;
            buckets[k].3 += b * wgt;
        }
        i += step;
    }
    let best = buckets.iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())?;
    let (r, g, b) = if best.0 > 0.0 { (best.1 / best.0, best.2 / best.0, best.3 / best.0) } else { (0.55, 0.55, 0.6) };
    // Normalize brightness: scale so the max channel is ~0.85 for good contrast on dark bg.
    let m = r.max(g).max(b).max(1e-6);
    let k = 0.85 / m;
    let to = |v: f64| ((v * k).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_spotify_cdns() {
        assert!(resolve("/i/ab67616d0000b273").is_some());
        assert!(resolve("/i/../../etc").is_none());
        let enc = |s: &str| {
            const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let d = s.as_bytes();
            let mut o = String::new();
            for c in d.chunks(3) {
                let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
                let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
                o.push(T[(n >> 18) as usize & 63] as char);
                o.push(T[(n >> 12) as usize & 63] as char);
                if c.len() > 1 { o.push(T[(n >> 6) as usize & 63] as char) }
                if c.len() > 2 { o.push(T[n as usize & 63] as char) }
            }
            o
        };
        assert!(resolve(&format!("/x/{}", enc("https://mosaic.scdn.co/640/abc"))).is_some());
        assert!(resolve(&format!("/x/{}", enc("https://evil.com/a.png"))).is_none());
        assert!(resolve(&format!("/x/{}", enc("https://evil.com@x.scdn.co/a"))).is_none());
        assert!(resolve(&format!("/x/{}", enc("http://x.scdn.co/a"))).is_none());
    }
}
