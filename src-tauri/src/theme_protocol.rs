//! Serves theme documents on `theme://` (http://theme.localhost on Windows).
//!
//! Every response carries a strict CSP header. The host page embeds the frame
//! with `sandbox="allow-scripts"` (opaque origin, no IPC) and only allows this
//! origin in `frame-src`, so a theme can neither reach the network nor navigate
//! itself somewhere that could.

use tauri::http::{Request, Response};
use tauri::AppHandle;

pub const THEME_CSP: &str = "default-src 'none'; \
    script-src http://theme.localhost theme: 'unsafe-inline'; \
    style-src http://theme.localhost theme: 'unsafe-inline'; \
    img-src http://theme.localhost theme: http://img.localhost img: data: blob:; \
    font-src http://theme.localhost theme: data:; \
    media-src 'none'; connect-src 'none'; frame-src 'none'; worker-src 'none'; \
    object-src 'none'; base-uri 'none'; form-action 'none'";

pub fn handle(_app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path();
    match path {
        "/frame.html" => respond("text/html; charset=utf-8", PLACEHOLDER_FRAME.as_bytes().to_vec()),
        _ => Response::builder()
            .status(404)
            .header("Content-Security-Policy", THEME_CSP)
            .body(Vec::new())
            .unwrap(),
    }
}

fn respond(mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .header("Content-Type", mime)
        .header("Content-Security-Policy", THEME_CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap()
}

// Stage 0 placeholder; replaced by the real theme loader in stage 4.
const PLACEHOLDER_FRAME: &str = r#"<!doctype html>
<html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#121212;color:#eee;font:16px system-ui;display:grid;place-items:center}</style>
</head><body><div id="m">mp3palace theme frame</div>
<script>
  parent.postMessage({ type: "theme:hello", apiVersion: 1 }, "*");
  addEventListener("message", (e) => {
    if (e.data && e.data.type === "host:ping") parent.postMessage({ type: "theme:pong", id: e.data.id }, "*");
  });
</script></body></html>"#;
