// Host page: loads the active theme into the sandbox and relays messages.
(() => {
  const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
  const frame = document.getElementById("theme");
  const THEME_ORIGIN = "http://theme.localhost";
  let ready = false;

  window.addEventListener("message", (e) => {
    if (e.source !== frame.contentWindow) return;
    const msg = e.data;
    if (!msg || typeof msg.type !== "string") return;
    if (msg.type === "theme:hello" && !ready) {
      ready = true;
      invoke("host_ready", { via: "hello" });
    }
  });

  frame.src = THEME_ORIGIN + "/frame.html";
  // Never leave the window hidden if the theme fails to say hello.
  setTimeout(() => { if (!ready) { ready = true; invoke("host_ready", { via: "timeout" }); } }, 3000);
})();
