// Spectrum theme — layer 4 example. Uses only the public `mp3` API.
(() => {
  const STYLES = ["bars", "mirror", "wave"];
  let style = "bars";
  mp3.storage.get("style").then((s) => { if (STYLES.includes(s)) style = s; });

  // 1) A spectrum behind the player bar (re-attached whenever the bar re-renders).
  function attachBar() {
    const bar = mp3.ui.region("player");
    if (!bar || bar.querySelector(".spx-canvas")) return;
    const c = document.createElement("canvas");
    c.className = "spx-canvas";
    const f = document.createElement("div");
    f.className = "spx-flash";
    bar.prepend(c, f);
  }
  new MutationObserver(attachBar).observe(document.getElementById("mp3-root"), { childList: true, subtree: true });

  // 2) Draw on every analysis frame (~60 Hz while playing and visible).
  let last = null;
  function draw(canvas, frame, kind) {
    const dpr = devicePixelRatio || 1;
    const w = (canvas.width = canvas.clientWidth * dpr);
    const h = (canvas.height = canvas.clientHeight * dpr);
    const g = canvas.getContext("2d");
    g.clearRect(0, 0, w, h);
    const bins = frame.bins;
    const n = bins.length;
    const grad = g.createLinearGradient(0, h, w, 0);
    grad.addColorStop(0, "#00e5ff");
    grad.addColorStop(1, "#ff3dcb");
    g.fillStyle = grad;
    g.strokeStyle = grad;
    if (kind === "wave") {
      g.lineWidth = 3 * dpr;
      g.beginPath();
      for (let i = 0; i < n; i++) {
        const x = (i / (n - 1)) * w;
        const y = h / 2 - (bins[i] - 0.3) * h * 0.8 * (i % 2 ? 1 : -1);
        i ? g.lineTo(x, y) : g.moveTo(x, y);
      }
      g.stroke();
      return;
    }
    const bw = w / n;
    for (let i = 0; i < n; i++) {
      const v = Math.max(0, bins[i] - 0.15) / 0.85;
      const bh = v * h * (kind === "mirror" ? 0.5 : 0.95);
      if (kind === "mirror") g.fillRect(i * bw + 1, h / 2 - bh, bw - 2, bh * 2);
      else g.fillRect(i * bw + 1, h - bh, bw - 2, bh);
    }
  }
  mp3.audio.subscribe((frame) => {
    last = frame;
    const bar = mp3.ui.region("player");
    const c = bar && bar.querySelector(".spx-canvas");
    if (c) draw(c, frame, "bars");
    const flash = bar && bar.querySelector(".spx-flash");
    if (flash && frame.beat) {
      flash.classList.add("on");
      setTimeout(() => flash.classList.remove("on"), 60);
    }
    const big = document.querySelector(".spx-big");
    if (big) draw(big, frame, style);
  });

  // 3) A full-screen visualizer view, reachable from the command bar.
  mp3.ui.registerView("visualizer", async () =>
    mp3.ui.render("view-visualizer", { track: mp3.nowPlaying.track, style })
  );
  mp3.ui.registerCommand({ title: "Open visualizer", hint: "Spectrum", run: () => mp3.nav.open("visualizer") });
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".spx-view")) return;
    style = STYLES[(STYLES.indexOf(style) + 1) % STYLES.length];
    mp3.storage.set("style", style);
    const hint = document.querySelector(".spx-hint");
    if (hint) hint.textContent = `Click to switch style (${style}) · Esc / Back to leave`;
  });
  mp3.on("trackChanged", (s) => {
    const t = document.querySelector(".spx-title");
    if (t && s.track) t.textContent = s.track.title;
  });
  window.__spectrumLoaded = true;
})();
