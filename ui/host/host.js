// openmp3 host page.
//
// The ONLY trusted page. It loads the active theme into a sandboxed iframe
// (opaque origin, strict CSP, no IPC), relays whitelisted commands from the
// theme to the engine bridge and events back, runs the watchdog, and falls
// back to Default when a theme breaks. It never renders UI itself apart from
// the fallback banner.
import { invoke, Channel } from "./vendor/tauri/core.js";
import { listen } from "./vendor/tauri/event.js";

const ORIGINS = ["http://theme.localhost", "http://themeb.localhost"];
let originIdx = 0;
let frame = document.getElementById("theme");
const MINI = new URLSearchParams(location.search).get("mini") === "1";
const banner = document.getElementById("host-banner");

let whitelist = new Set();
let themeId = "default";
let shown = false;
let alive = false;
let loadAt = 0;
let lastPong = 0;
let pingSeq = 0;
let errors = [];
let failed = false;
let lastPerf = 0;
let hungRecovery = false;
let token = ""; // host token: never leaves this module

const MAX_ARGS_BYTES = 600 * 1024;

function showWindow(via) {
  if (shown) return;
  shown = true;
  invoke("host_ready", { via }).catch(() => {});
}

function showBanner(text, ms = 12000) {
  banner.textContent = "";
  const span = document.createElement("span");
  span.textContent = text;
  const close = document.createElement("button");
  close.textContent = "×";
  close.title = "Dismiss";
  close.onclick = () => (banner.hidden = true);
  banner.append(span, close);
  banner.hidden = false;
  if (ms) setTimeout(() => (banner.hidden = true), ms);
}

function post(msg, transfer) {
  if (!frame.contentWindow) return;
  frame.contentWindow.postMessage(Object.assign({ mp3: 1 }, msg), "*", transfer || []);
}

function mount(id, reason, fresh) {
  if (fresh) {
    // Replace the iframe element and switch origin so Chromium gives the theme
    // a new renderer process (a hung one can't be reused).
    originIdx = 1 - originIdx;
    const next = frame.cloneNode(false);
    next.removeAttribute("src");
    frame.replaceWith(next);
    frame = next;
  }
  themeId = id;
  alive = false;
  failed = false;
  errors = [];
  loadAt = performance.now();
  lastPong = loadAt;
  // Cache-buster so live-link edits always reload.
  frame.src = `${ORIGINS[originIdx]}/frame/${encodeURIComponent(id)}?v=${Date.now()}${MINI ? "&mini=1" : ""}`;
  if (reason) showBanner(reason);
}

function fail(reason) {
  if (failed) return;
  failed = true;
  if (themeId === "default") {
    // Default itself misbehaving: reload it once rather than loop.
    showBanner("The UI stopped responding and was reloaded. " + reason, 0);
    setTimeout(() => mount("default", null, true), 300);
    return;
  }
  hungRecovery = true;
  invoke("theme_failed", { id: themeId, reason, token }).catch(() => mount("default", reason, true));
}

// ------------------------------------------------------------------ theme → host
window.addEventListener("message", async (e) => {
  if (e.source !== frame.contentWindow) return;
  const m = e.data;
  if (!m || m.mp3 !== 1 || typeof m !== "object") return;

  if (m.hello) {
    alive = true;
    lastPong = performance.now();
    return;
  }
  if (m.ready) {
    alive = true;
    showWindow("hello");
    return;
  }
  if (m.pong !== undefined) {
    lastPong = performance.now();
    if (m.perf && performance.now() - lastPerf > 2000) {
      lastPerf = performance.now();
      invoke("perf_report", { sample: m.perf, token }).catch(() => {});
    }
    return;
  }
  if (m.themeError) {
    const now = performance.now();
    errors.push(now);
    errors = errors.filter((t) => now - t < 10000);
    console.warn("[theme error]", m.themeError);
    // Crash loop right after load → treat as broken.
    if (errors.length > 25 && now - loadAt < 15000) fail("too many script errors");
    return;
  }
  if (typeof m.id !== "number" || typeof m.cmd !== "string") return;

  // Layer 1 validation (layer 2 is engine-bridge in Rust).
  const reply = (ok, payload) => post(ok ? { id: m.id, ok: true, result: payload } : { id: m.id, ok: false, error: payload });
  if (!whitelist.has(m.cmd)) return reply(false, { code: "unknown_command", message: `unknown command \`${String(m.cmd).slice(0, 64)}\`` });
  const args = m.args === undefined ? {} : m.args;
  if (args === null || typeof args !== "object" || Array.isArray(args)) return reply(false, { code: "invalid", message: "args must be an object" });
  let size = 0;
  try {
    size = JSON.stringify(args).length;
  } catch {
    return reply(false, { code: "invalid", message: "args not serializable" });
  }
  if (size > MAX_ARGS_BYTES) return reply(false, { code: "invalid", message: "args too large" });

  try {
    const result = await invoke("bridge_call", { cmd: m.cmd, args, themeId, token });
    reply(true, result === undefined ? null : result);
  } catch (err) {
    const e2 = typeof err === "object" && err ? err : { code: "failed", message: String(err) };
    reply(false, { code: e2.code || "failed", message: e2.message || String(err) });
  }
});

// ------------------------------------------------------------------ engine → theme
listen("bridge:event", (ev) => {
  const p = ev.payload || {};
  if (typeof p.event === "string") post({ event: p.event, data: p.data });
});
listen("host:reload-theme", (ev) => {
  const p = ev.payload || {};
  const fresh = hungRecovery;
  hungRecovery = false;
  mount(p.id || "default", p.reason, fresh);
});

// Audio analysis frames: binary from Rust, transferred (zero-copy) into the frame.
const audio = new Channel((msg) => {
  let buf = msg instanceof ArrayBuffer ? msg : Array.isArray(msg) ? new Float32Array(new Uint8Array(msg).buffer).buffer : null;
  if (!buf) return;
  post({ audio: buf }, [buf]);
});


// ------------------------------------------------------------------ watchdog
setInterval(() => {
  if (failed || document.hidden) return;
  const now = performance.now();
  if (!alive && now - loadAt > 6000) return fail("it didn't start (no response within 6 s)");
  if (alive && now - lastPong > 10000) return fail("it stopped responding (no heartbeat for 10 s)");
  post({ ping: ++pingSeq });
}, 3000);

frame.addEventListener("load", () => {
  // A theme document that failed server-side shows no hello → watchdog fires.
  loadAt = performance.now();
});

// ------------------------------------------------------------------ boot
(async () => {
  try {
    const init = await invoke("host_init");
    token = init.token || "";
    if (!token) showBanner("Host token unavailable — reload the window (Ctrl+R).", 0);
    whitelist = new Set(init.commands || []);
    invoke("audio_channel", { channel: audio, token }).catch((e) => console.warn("audio channel", e));
    mount(init.themeId || "default", init.fallbackReason);
  } catch (e) {
    console.error(e);
    mount("default", "Startup problem: " + e);
  }
  // Never leave the window hidden if the theme is slow or broken.
  setTimeout(() => showWindow("timeout"), 2500);
})();
