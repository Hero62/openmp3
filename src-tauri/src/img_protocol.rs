//! `img://` (http://img.localhost) — local, cached proxy for Spotify cover art.
//!
//! Themes may only load images from this origin (their CSP forbids remote
//! URLs). Only Spotify CDN hosts are proxied:
//! * `/i/<hex>`        → https://i.scdn.co/image/<hex>
//! * `/x/<base64url>`  → any https URL on *.scdn.co / *.spotifycdn.com
//!
//! Also computes, per cover, an accent color (for `accentFromCover` themes) and a
//! palette of its main colors (for themes that want the real cover colors).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use tauri::http::{Request, Response};

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

const MAX_CACHE_BYTES: u64 = 300 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub fn init(cache_dir: PathBuf) {
    let _ = std::fs::create_dir_all(&cache_dir);
    let _ = CACHE_DIR.set(cache_dir.clone());
    let _ = CLIENT.set(
        reqwest::Client::builder()
            .user_agent(concat!("openmp3/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(15))
            // A redirect could leave the CDN allowlist; covers never redirect.
            .redirect(reqwest::redirect::Policy::none())
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
        let raw = String::from_utf8(b64url_decode(enc)?).ok()?;
        let url = allowed_cdn_url(&raw)?;
        return Some((format!("x_{}", sha256_hex(url.as_bytes())), url));
    }
    None
}

/// Parses `raw` the same way the HTTP client will (WHATWG URL rules) and
/// returns the normalized URL only if it is plain https on a Spotify CDN host.
fn allowed_cdn_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = match url.host()? {
        url::Host::Domain(h) => h.to_ascii_lowercase(),
        _ => return None,
    };
    let ok = ["scdn.co", "spotifycdn.com"]
        .iter()
        .any(|d| host.strip_suffix(d).map(|p| p.ends_with('.') && p.len() > 1).unwrap_or(false));
    ok.then(|| url.to_string())
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub async fn fetch(path: &str) -> Option<Arc<Vec<u8>>> {
    let (name, url) = resolve(path)?;
    let dir = CACHE_DIR.get()?;
    let file = dir.join(&name);
    if let Ok(b) = tokio::fs::read(&file).await {
        return Some(Arc::new(b));
    }
    let mut resp = CLIENT.get()?.get(&url).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().unwrap_or(0) > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    // Read with a running cap so an endless body can't exhaust memory.
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
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
                .header("X-Content-Type-Options", "nosniff")
                .body(b.to_vec())
                .unwrap(),
            None => Response::builder().status(404).body(Vec::new()).unwrap(),
        };
        responder.respond(resp);
    });
}

/// Colors of a cover: the accent (`#rrggbb`) and a palette of up to 5 main colors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoverColors {
    pub accent: Option<String>,
    pub palette: Vec<String>,
}

/// Cover colors for a local proxy URL like `http://img.localhost/i/<hex>`.
pub async fn cover_colors(local_url: &str) -> CoverColors {
    let Some(path) = local_url.strip_prefix("http://img.localhost").or_else(|| local_url.strip_prefix("img://localhost")) else {
        return CoverColors::default();
    };
    let Some(bytes) = fetch(path).await else { return CoverColors::default() };
    tokio::task::spawn_blocking(move || {
        let Some((px, w, h)) = decode_rgb(&bytes) else { return CoverColors::default() };
        CoverColors { accent: accent_from_rgb(&px, w, h), palette: palette_from_rgb(&px, w, h, 5) }
    })
    .await
    .unwrap_or_default()
}

fn decode_rgb(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let px = dec.decode().ok()?;
    let (w, h) = dec.dimensions()?;
    (w > 0 && h > 0 && px.len() >= w * h * 3).then_some((px, w, h))
}

/// The cover's main colors, most prominent first (`#rrggbb`, at most `max`).
/// Box splitting (median-cut style) over a ~4k pixel sample, then near-duplicates are merged, so a
/// two-tone cover gives two colors and a black-and-white one gives grays.
pub fn palette_from_rgb(px: &[u8], w: usize, h: usize, max: usize) -> Vec<String> {
    let step = ((w * h) / 4096).max(1);
    let mut pts: Vec<[u8; 3]> = (0..w * h).step_by(step).map(|i| [px[i * 3], px[i * 3 + 1], px[i * 3 + 2]]).collect();
    if pts.is_empty() {
        return Vec::new();
    }
    // Split the box with the widest channel range (weighted by size) at its median.
    let mut boxes: Vec<(usize, usize)> = vec![(0, pts.len())];
    let range = |p: &[[u8; 3]]| -> (usize, u8) {
        (0..3)
            .map(|c| {
                let (lo, hi) = p.iter().fold((255u8, 0u8), |(lo, hi), x| (lo.min(x[c]), hi.max(x[c])));
                (c, hi - lo)
            })
            .max_by_key(|&(_, r)| r)
            .unwrap()
    };
    while boxes.len() < 8 {
        let (bi, &(s, e)) = boxes
            .iter()
            .enumerate()
            .max_by_key(|(_, &(s, e))| range(&pts[s..e]).1 as usize * (e - s))
            .unwrap();
        if e - s < 2 || range(&pts[s..e]).1 < 8 {
            break;
        }
        let (c, _) = range(&pts[s..e]);
        pts[s..e].sort_unstable_by_key(|x| x[c]);
        // Cut at the middle of the value range, not the median: a median cut can land
        // inside one color and average two distinct colors into a muddy one.
        let mid = (pts[s][c] as u16 + pts[e - 1][c] as u16) / 2;
        let m = s + pts[s..e].partition_point(|x| (x[c] as u16) <= mid).clamp(1, e - s - 1);
        boxes[bi] = (s, m);
        boxes.push((m, e));
    }
    // Average each box, merge colors that look the same, keep the biggest.
    let mut cols: Vec<([f64; 3], usize)> = boxes
        .iter()
        .map(|&(s, e)| {
            let n = (e - s) as f64;
            let sum = pts[s..e].iter().fold([0f64; 3], |a, x| [a[0] + x[0] as f64, a[1] + x[1] as f64, a[2] + x[2] as f64]);
            ([sum[0] / n, sum[1] / n, sum[2] / n], e - s)
        })
        .collect();
    cols.sort_by(|a, b| b.1.cmp(&a.1));
    let mut out: Vec<([f64; 3], usize)> = Vec::new();
    for (c, n) in cols {
        let close = out.iter_mut().find(|(o, _)| ((o[0] - c[0]).powi(2) + (o[1] - c[1]).powi(2) + (o[2] - c[2]).powi(2)).sqrt() < 28.0);
        match close {
            Some((o, on)) => {
                let t = n as f64 / (*on + n) as f64;
                for k in 0..3 {
                    o[k] += (c[k] - o[k]) * t;
                }
                *on += n;
            }
            None => out.push((c, n)),
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out.iter()
        .take(max)
        .map(|(c, _)| format!("#{:02x}{:02x}{:02x}", c[0].round() as u8, c[1].round() as u8, c[2].round() as u8))
        .collect()
}

/// Picks the most common vivid hue, then nudges lightness so it reads on dark UIs.
pub fn accent_from_rgb(px: &[u8], w: usize, h: usize) -> Option<String> {
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

    fn solid(cols: &[([u8; 3], usize)]) -> (Vec<u8>, usize, usize) {
        let mut px = Vec::new();
        for (c, n) in cols {
            for _ in 0..*n {
                px.extend_from_slice(c);
            }
        }
        let w = px.len() / 3;
        (px, w, 1)
    }

    #[test]
    fn palette_follows_the_cover() {
        // Mostly dark navy with an orange accent: navy first, orange second.
        let (px, w, h) = solid(&[([20, 24, 60], 7000), ([240, 120, 30], 3000)]);
        assert_eq!(palette_from_rgb(&px, w, h, 5), vec!["#14183c", "#f0781e"]);
        // Black and white stays black and white (no invented hue).
        let (px, w, h) = solid(&[([0, 0, 0], 5000), ([250, 250, 250], 5000)]);
        let p = palette_from_rgb(&px, w, h, 5);
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|c| c[1..3] == c[3..5] && c[3..5] == c[5..7]), "{p:?}");
        // Near-identical shades merge into one color.
        let (px, w, h) = solid(&[([200, 30, 30], 5000), ([205, 34, 28], 5000)]);
        assert_eq!(palette_from_rgb(&px, w, h, 5).len(), 1);
    }

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
        // Host-confusion tricks: the client would connect to evil.tld.
        for bad in [
            "https://evil.tld\\x.scdn.co/a",
            "https://evil.tld#.scdn.co",
            "https://evil.tld?.scdn.co",
            "https://evil.tld/.scdn.co",
            "https://x.scdn.co:8443/a",
            "https://192.168.1.1/x.scdn.co",
            "https://scdn.co/a",
            "https://evilscdn.co/a",
        ] {
            assert!(resolve(&format!("/x/{}", enc(bad))).is_none(), "{bad}");
        }
        // Cache names come from the full URL, so long URLs never collide.
        let a = resolve(&format!("/x/{}", enc(&format!("https://mosaic.scdn.co/640/{}1", "a".repeat(200))))).unwrap();
        let b = resolve(&format!("/x/{}", enc(&format!("https://mosaic.scdn.co/640/{}2", "a".repeat(200))))).unwrap();
        assert_ne!(a.0, b.0);
    }

    /// Prints palettes for real covers: `PALETTE_DIR=<dir> cargo test palette_dump -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn palette_dump() {
        let dir = std::env::var("PALETTE_DIR").expect("PALETTE_DIR");
        for e in std::fs::read_dir(dir).unwrap().flatten().take(24) {
            let b = std::fs::read(e.path()).unwrap();
            if let Some((px, w, h)) = decode_rgb(&b) {
                println!("PAL {} {} {}", e.path().display(), accent_from_rgb(&px, w, h).unwrap_or_default(), palette_from_rgb(&px, w, h, 5).join(","));
            }
        }
    }

}
