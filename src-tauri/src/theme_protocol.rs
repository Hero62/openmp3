//! Serves theme documents on `theme://` (http://theme.localhost on Windows).
//!
//! * `/frame/<id>`      the sandboxed document for a theme
//! * `/rt/<file>`       the shared theme runtime (runtime.js, base.css)
//! * `/t/<id>/<path>`   a theme's own files (theme.css, script.js, assets/*)
//!
//! Every response carries a strict CSP header. The host page embeds the frame
//! with `sandbox="allow-scripts"` (opaque origin → no Tauri IPC, no storage,
//! no cookies) and only allows this origin in `frame-src`, so a theme can
//! neither reach the network nor navigate itself somewhere that could.

use serde_json::json;
use tauri::{
    http::{Request, Response},
    AppHandle, Manager,
};

use crate::{app::State, themes};

pub const THEME_CSP: &str = "default-src 'none'; \
    script-src http://theme.localhost theme: http://themeb.localhost themeb:; \
    style-src http://theme.localhost theme: http://themeb.localhost themeb: 'unsafe-inline'; \
    img-src http://theme.localhost theme: http://themeb.localhost themeb: http://img.localhost img: data: blob:; \
    font-src http://theme.localhost theme: http://themeb.localhost themeb: data:; \
    media-src 'none'; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; \
    object-src 'none'; base-uri 'none'; form-action 'none'; \
    frame-ancestors http://tauri.localhost tauri://localhost";

fn respond(status: u16, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", THEME_CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer")
        .body(body)
        .unwrap()
}

fn not_found() -> Response<Vec<u8>> {
    respond(404, "text/plain", b"not found".to_vec())
}

pub fn handle(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let state = app.state::<State>();
    let path = request.uri().path();
    let path = percent_decode(path);

    if let Some(id) = path.strip_prefix("/frame/") {
        return match state.themes.load(id) {
            Ok(t) => {
                let boot = json!({
                    "apiVersion": engine_bridge::API_VERSION,
                    "theme": { "id": t.id, "meta": t.meta, "layout": t.layout, "hasScript": t.has_script },
                    "commands": engine_bridge::COMMAND_NAMES,
                    "events": engine_bridge::EVENTS,
                });
                respond(200, "text/html; charset=utf-8", state.themes.frame_html(&t, &boot).into_bytes())
            }
            Err(e) => {
                // The host notices the missing hello and falls back to Default.
                respond(500, "text/plain; charset=utf-8", format!("theme failed to load: {e:#}").into_bytes())
            }
        };
    }
    if let Some(rel) = path.strip_prefix("/rt/") {
        return match state.themes.runtime_file(rel) {
            Some(b) => respond(200, themes::mime_for(rel), b),
            None => not_found(),
        };
    }
    if let Some(rest) = path.strip_prefix("/t/") {
        let Some((id, rel)) = rest.split_once('/') else { return not_found() };
        if rel == "theme.json" || rel == "layout.json" || rel.starts_with("components/") {
            return not_found(); // inlined into the frame; never fetched by themes
        }
        return match state.themes.file(id, rel) {
            Ok(Some(b)) => respond(200, themes::mime_for(rel), b),
            _ => not_found(),
        };
    }
    not_found()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
