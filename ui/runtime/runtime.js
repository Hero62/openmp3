/* mp3palace theme runtime (API v1).
 *
 * Runs inside the sandboxed theme frame for EVERY theme, including Default.
 * It has no special powers: everything goes through postMessage to the host,
 * which validates each command against the bridge whitelist.
 *
 * Provides: the `mp3` SDK, a tiny template engine ({{…}} placeholders, no
 * eval), the layout grid from layout.json, the router + views, virtualized
 * lists, data-action handling, keyboard shortcuts, the Ctrl+K command bar,
 * context menus, lyrics sync, and the Settings / Themes pages.
 */
(() => {
  "use strict";
  const BOOT = JSON.parse(document.getElementById("mp3-boot").textContent);
  const API_VERSION = 1;
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  // ------------------------------------------------------------------ bridge
  let seq = 0;
  const pending = new Map();
  const listeners = new Map();
  const audioSubs = new Set();
  function post(msg) {
    parent.postMessage(Object.assign({ mp3: 1 }, msg), "*");
  }
  function call(cmd, args) {
    return new Promise((resolve, reject) => {
      const id = ++seq;
      pending.set(id, { resolve, reject, cmd });
      post({ id, cmd, args: args || {} });
      setTimeout(() => {
        if (pending.has(id)) {
          pending.delete(id);
          reject(new Error(cmd + ": timed out"));
        }
      }, 60000);
    });
  }
  function emit(event, data) {
    (listeners.get(event) || []).forEach((cb) => {
      try {
        cb(data);
      } catch (e) {
        console.error(e);
        reportError(e);
      }
    });
  }
  window.addEventListener("message", (e) => {
    if (e.source !== parent) return;
    const m = e.data;
    if (!m || m.mp3 !== 1) return;
    if (m.ping !== undefined) return post({ pong: m.ping, perf: perfSample() });
    if (m.id !== undefined && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      if (m.ok) p.resolve(m.result);
      else p.reject(Object.assign(new Error((m.error && m.error.message) || "error"), { code: m.error && m.error.code }));
      return;
    }
    if (m.audio) {
      const f = new Float32Array(m.audio);
      const frame = { rms: f[0], beat: f[1] > 0.5, bins: f.subarray(2) };
      audioSubs.forEach((cb) => {
        try {
          cb(frame);
        } catch (err) {
          reportError(err);
        }
      });
      return;
    }
    if (m.event) emit(m.event, m.data);
  });
  function reportError(e) {
    post({ themeError: String((e && e.stack) || e).slice(0, 2000) });
  }
  window.addEventListener("error", (e) => reportError(e.error || e.message));
  window.addEventListener("unhandledrejection", (e) => console.warn("unhandled", e.reason));

  // frame-time sampling for the performance meter
  let frameTimes = [];
  let lastFrame = performance.now();
  (function tick(t) {
    frameTimes.push(t - lastFrame);
    if (frameTimes.length > 120) frameTimes.shift();
    lastFrame = t;
    requestAnimationFrame(tick);
  })(lastFrame);
  function perfSample() {
    const ft = frameTimes.slice().sort((a, b) => a - b);
    const mem = performance.memory ? performance.memory.usedJSHeapSize : 0;
    return {
      frameMs: ft.length ? ft[Math.floor(ft.length / 2)] : 0,
      frameP95: ft.length ? ft[Math.floor(ft.length * 0.95)] : 0,
      heap: mem,
      nodes: document.getElementsByTagName("*").length,
    };
  }

  // ------------------------------------------------------------------ state
  const S = {
    playback: null,
    queue: null,
    playlists: [],
    likedSet: new Set(),
    settings: null,
    session: null,
    lyrics: null,
    route: null,
    history: [],
    forward: [],
    scroll: new Map(),
    panel: "queue",
    sidebarCollapsed: false,
    compact: false,
  };

  // ------------------------------------------------------------------ template engine
  const tplCache = new Map();
  function parseTemplate(src) {
    const root = { type: "root", children: [] };
    const stack = [root];
    const re = /\{\{([#\/>^]?)\s*([^}]*?)\s*\}\}/g;
    let last = 0,
      m;
    while ((m = re.exec(src))) {
      if (m.index > last) stack[stack.length - 1].children.push({ type: "text", v: src.slice(last, m.index) });
      last = re.lastIndex;
      const [, sig, body] = m;
      const top = stack[stack.length - 1];
      if (sig === "#") {
        const [kind, ...rest] = body.split(/\s+/);
        const node = { type: kind, expr: rest.join(" "), children: [], alt: null };
        top.children.push(node);
        stack.push(node);
      } else if (sig === "/") {
        if (stack.length > 1) stack.pop();
      } else if (sig === ">") {
        const [name, ...rest] = body.split(/\s+/);
        top.children.push({ type: "partial", name, expr: rest.join(" ") });
      } else if (body === "else") {
        top.alt = [];
        top.children = top.children; // keep
        top._inElse = true;
        top.elseStart = top.children.length;
      } else {
        top.children.push({ type: "var", expr: body });
      }
    }
    if (last < src.length) stack[0].children.push({ type: "text", v: src.slice(last) });
    // split if/else children
    (function fix(n) {
      if (n.elseStart !== undefined) {
        n.alt = n.children.slice(n.elseStart);
        n.children = n.children.slice(0, n.elseStart);
      }
      (n.children || []).forEach(fix);
      (n.alt || []).forEach(fix);
    })(root);
    return root;
  }
  function lookup(ctx, path) {
    path = path.trim();
    if (path === "this" || path === ".") return ctx.self;
    if (path.startsWith("@")) return ctx.meta[path.slice(1)];
    if (/^'.*'$|^".*"$/.test(path)) return path.slice(1, -1);
    if (/^-?\d+(\.\d+)?$/.test(path)) return Number(path);
    const parts = path.replace(/^this\./, "").split(".");
    for (let c = ctx; c; c = c.parent) {
      let v = c.self;
      if (v != null && typeof v === "object" && parts[0] in v) {
        for (const p of parts) v = v == null ? undefined : v[p];
        return v;
      }
      if (c.parent == null && c.globals && parts[0] in c.globals) {
        let g = c.globals;
        for (const p of parts) g = g == null ? undefined : g[p];
        return g;
      }
    }
    return undefined;
  }
  const FILTERS = {
    duration: (v) => fmtTime(v),
    time: (v) => fmtTime(v),
    artists: (v) => (Array.isArray(v) ? v.map((a) => a.name).join(", ") : ""),
    img: (v, size) => pickImage(v, Number(size) || 300),
    date: (v) => fmtDate(v),
    count: (v) => (typeof v === "number" ? v.toLocaleString() : v),
    upper: (v) => String(v ?? "").toUpperCase(),
    lower: (v) => String(v ?? "").toLowerCase(),
    default: (v, d) => (v == null || v === "" ? d : v),
    plural: (v, word) => `${v} ${word}${v === 1 ? "" : "s"}`,
    first: (v) => (Array.isArray(v) ? v[0] : v),
    length: (v) => (v ? v.length : 0),
    json: (v) => JSON.stringify(v),
    percent: (v) => Math.round((Number(v) || 0) * 100),
    not: (v) => !v,
    eq: (v, x) => String(v) === String(x),
    year: (v) => v || "",
  };
  function evalExpr(ctx, expr) {
    const [head, ...filters] = expr.split("|").map((s) => s.trim());
    let v = lookup(ctx, head);
    for (const f of filters) {
      const [name, ...args] = f.split(":").map((s) => s.trim().replace(/^'(.*)'$/, "$1"));
      const fn = FILTERS[name];
      if (fn) v = fn(v, ...args);
    }
    return v;
  }
  const ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ESC[c]);
  function truthy(v) {
    return Array.isArray(v) ? v.length > 0 : !!v;
  }
  function renderNodes(nodes, ctx, out) {
    for (const n of nodes) {
      switch (n.type) {
        case "text":
          out.push(n.v);
          break;
        case "var": {
          const v = evalExpr(ctx, n.expr);
          out.push(esc(v === undefined || v === null || v === false ? "" : v));
          break;
        }
        case "if":
          renderNodes(truthy(evalExpr(ctx, n.expr)) ? n.children : n.alt || [], ctx, out);
          break;
        case "unless":
          renderNodes(!truthy(evalExpr(ctx, n.expr)) ? n.children : n.alt || [], ctx, out);
          break;
        case "each": {
          const list = evalExpr(ctx, n.expr);
          if (Array.isArray(list) && list.length) {
            list.forEach((item, i) =>
              renderNodes(n.children, { self: item, parent: ctx, meta: { index: i, number: i + 1, first: i === 0, last: i === list.length - 1 } }, out)
            );
          } else renderNodes(n.alt || [], ctx, out);
          break;
        }
        case "with": {
          const v = evalExpr(ctx, n.expr);
          if (v != null) renderNodes(n.children, { self: v, parent: ctx, meta: ctx.meta }, out);
          break;
        }
        case "partial": {
          const data = n.expr ? evalExpr(ctx, n.expr) : ctx.self;
          out.push(render(n.name, data, ctx));
          break;
        }
      }
    }
  }
  function getTemplate(name) {
    if (!tplCache.has(name)) {
      const src = (BOOT.components || {})[name];
      tplCache.set(name, typeof src === "string" ? parseTemplate(src) : null);
    }
    return tplCache.get(name);
  }
  function render(name, data, parentCtx) {
    const t = getTemplate(name);
    if (!t) return `<!-- missing component ${esc(name)} -->`;
    const out = [];
    renderNodes(t.children, { self: data, parent: parentCtx || null, meta: (parentCtx && parentCtx.meta) || {}, globals: globals() }, out);
    return out.join("");
  }
  function globals() {
    const pb = S.playback || {};
    return {
      now: pb,
      track: pb.track,
      queue: S.queue,
      settings: S.settings,
      session: S.session,
      playlists: S.playlists,
      route: S.route,
      panel: S.panel,
      theme: BOOT.theme,
    };
  }

  // ------------------------------------------------------------------ helpers
  function fmtTime(ms) {
    ms = Math.max(0, Number(ms) || 0);
    const s = Math.floor(ms / 1000);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = String(s % 60).padStart(2, "0");
    return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
  }
  function fmtDate(secs) {
    if (!secs) return "";
    const d = new Date(secs * 1000);
    const diff = (Date.now() - d) / 86400000;
    if (diff < 1) return "today";
    if (diff < 2) return "yesterday";
    if (diff < 30) return `${Math.floor(diff)} days ago`;
    return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  }
  function pickImage(images, size) {
    if (!Array.isArray(images) || !images.length) return "";
    let best = images[0];
    for (const i of images) {
      if ((i.width || 300) >= size) {
        best = i;
        break;
      }
      best = i;
    }
    return best.url || "";
  }
  function debounce(fn, ms) {
    let t;
    return (...a) => {
      clearTimeout(t);
      t = setTimeout(() => fn(...a), ms);
    };
  }
  function toast(msg, kind = "info") {
    let host = $(".mp3-toasts");
    if (!host) {
      host = document.createElement("div");
      host.className = "mp3-toasts";
      document.body.appendChild(host);
    }
    const el = document.createElement("div");
    el.className = `mp3-toast mp3-toast-${kind}`;
    el.textContent = msg;
    host.appendChild(el);
    setTimeout(() => el.classList.add("mp3-toast-out"), 3200);
    setTimeout(() => el.remove(), 3700);
  }
  function positionNow() {
    const p = S.playback;
    if (!p) return 0;
    if (!p.playing) return p.positionMs || 0;
    return Math.min(p.durationMs || Infinity, (p.positionMs || 0) + (Date.now() - (p.positionAt || Date.now())));
  }
  const kindOf = (uri) => (uri || "").split(":")[1] || "";

  // ------------------------------------------------------------------ layout
  const REGIONS = ["topbar", "sidebar", "main", "panel", "player"];
  const REGION_COMPONENT = { topbar: "topbar", sidebar: "sidebar", main: null, panel: "right-panel", player: "player-bar" };
  function applyLayout() {
    const L = BOOT.theme.layout || {};
    const mode = S.compact && L.compact ? L.compact : L.normal;
    const root = $("#mp3-root");
    root.className = "mp3-app" + (S.compact ? " mp3-compact" : "") + (S.sidebarCollapsed ? " mp3-sidebar-collapsed" : "");
    const placed = new Set((mode.areas || []).join(" ").split(/\s+/));
    let areas = mode.areas.slice();
    let cols = mode.columns.slice();
    // Hidden panel / collapsed sidebar shrink their tracks.
    const hide = (name) => {
      const idx = areas[0].split(/\s+/).indexOf(name);
      if (idx >= 0 && areas.every((r) => r.split(/\s+/)[idx] === name)) {
        areas = areas.map((r) => r.split(/\s+/).filter((_, i) => i !== idx).join(" "));
        cols = cols.filter((_, i) => i !== idx);
      } else {
        areas = areas.map((r) => r.split(/\s+/).map((a) => (a === name ? "main" : a)).join(" "));
      }
      placed.delete(name);
    };
    if (S.panel === "none" && placed.has("panel")) hide("panel");
    if (S.sidebarCollapsed && placed.has("sidebar")) {
      const i = areas[0].split(/\s+/).indexOf("sidebar");
      if (i >= 0) cols[i] = "var(--sidebar-collapsed-w, 72px)";
    }
    root.style.gridTemplateAreas = areas.map((a) => `"${a}"`).join(" ");
    root.style.gridTemplateColumns = cols.join(" ");
    root.style.gridTemplateRows = mode.rows.join(" ");
    for (const r of REGIONS) {
      let el = root.querySelector(`[data-region="${r}"]`);
      const visible = placed.has(r) && (L.regions?.[r]?.visible ?? true);
      if (!visible) {
        if (el) el.remove();
        continue;
      }
      if (!el) {
        el = document.createElement(r === "main" ? "main" : r === "sidebar" ? "aside" : "section");
        el.dataset.region = r;
        el.className = `mp3-region mp3-${r}`;
        el.style.gridArea = r;
        root.appendChild(el);
      }
    }
    renderRegions();
  }
  function region(name) {
    return $(`[data-region="${name}"]`);
  }
  function renderRegions() {
    for (const r of ["topbar", "sidebar", "panel", "player"]) renderRegion(r);
    if (!region("main").firstChild) renderRoute();
  }
  function renderRegion(r) {
    const el = region(r);
    if (!el) return;
    const comp = REGION_COMPONENT[r];
    if (r === "panel") {
      if (S.panel === "none") return;
      el.innerHTML = render("right-panel", { panel: S.panel, queue: S.queue, lyrics: S.lyrics, track: S.playback && S.playback.track });
      if (S.panel === "lyrics") syncLyrics(true);
      return;
    }
    if (r === "sidebar") {
      el.innerHTML = render(comp, { playlists: S.playlists, route: S.route, collapsed: S.sidebarCollapsed });
      return;
    }
    el.innerHTML = render(comp, playerData());
    bindLive();
  }
  function playerData() {
    const p = S.playback || {};
    const t = p.track;
    return Object.assign({}, p, {
      liked: !!(t && S.likedSet.has(t.uri)),
      cover: t ? pickImage(t.album && t.album.images, 64) : "",
      volumePct: Math.round((p.volume ?? 0.8) * 100),
      shuffleOn: p.shuffle && p.shuffle !== "off",
      shuffleSpread: p.shuffle === "spread",
      repeatOn: p.repeat && p.repeat !== "off",
      repeatOne: p.repeat === "one",
      panelQueue: S.panel === "queue",
      panelLyrics: S.panel === "lyrics",
    });
  }

  // live-bound elements (progress, time) updated every frame without re-render
  let liveEls = [];
  function bindLive() {
    liveEls = $$("[data-bind]");
  }
  let lastLive = 0;
  function liveLoop(t) {
    requestAnimationFrame(liveLoop);
    if (document.hidden || t - lastLive < 200) return;
    lastLive = t;
    const pos = positionNow();
    const dur = (S.playback && S.playback.durationMs) || 0;
    for (const el of liveEls) {
      switch (el.dataset.bind) {
        case "position":
          el.textContent = fmtTime(pos);
          break;
        case "remaining":
          el.textContent = "-" + fmtTime(dur - pos);
          break;
        case "progress":
          el.style.setProperty("--progress", dur ? Math.min(1, pos / dur) : 0);
          if (el.tagName === "INPUT" && !el.matches(":active")) el.value = dur ? Math.round((pos / dur) * 1000) : 0;
          break;
      }
    }
    if (S.panel === "lyrics" || S.route?.view === "nowPlaying") syncLyrics(false);
  }
  requestAnimationFrame(liveLoop);

  // ------------------------------------------------------------------ router / views
  const VIEWS = {};
  function routeKey(r) {
    return r.view + (r.uri ? ":" + r.uri : "") + (r.query ? "?" + r.query : "");
  }
  function navigate(view, params = {}, opts = {}) {
    const main = region("main");
    if (S.route && main) S.scroll.set(routeKey(S.route), main.scrollTop);
    const r = Object.assign({ view }, params);
    if (S.route && !opts.replace && routeKey(S.route) !== routeKey(r)) {
      S.history.push(S.route);
      if (S.history.length > 50) S.history.shift();
      if (!opts.keepForward) S.forward = [];
    }
    S.route = r;
    renderRoute();
    renderRegion("sidebar");
    emit("navigate", r);
  }
  function back() {
    if (!S.history.length) return;
    S.forward.push(S.route);
    const r = S.history.pop();
    const main = region("main");
    if (S.route && main) S.scroll.set(routeKey(S.route), main.scrollTop);
    S.route = r;
    renderRoute();
    renderRegion("sidebar");
  }
  function forward() {
    if (!S.forward.length) return;
    const r = S.forward.pop();
    navigate(r.view, r, { keepForward: true });
  }
  let routeToken = 0;
  async function renderRoute() {
    const main = region("main");
    if (!main) return;
    const r = S.route || { view: "home" };
    const token = ++routeToken;
    const view = VIEWS[r.view] || VIEWS.home;
    main.dataset.view = r.view;
    try {
      const html = await view(r, (h) => {
        // allow progressive render (cached first)
        if (token === routeToken) setMain(h, r);
      });
      if (token !== routeToken) return;
      if (html !== undefined) setMain(html, r);
    } catch (e) {
      if (token !== routeToken) return;
      setMain(render("empty-state", { title: "Couldn't load this", message: e.message, retry: true }), r);
    }
  }
  function setMain(html, r) {
    const main = region("main");
    const key = routeKey(r);
    const keepScroll = main.dataset.key === key ? main.scrollTop : S.scroll.get(key) || 0;
    main.dataset.key = key;
    main.innerHTML = html;
    mountVirtualLists(main);
    main.scrollTop = keepScroll;
    updateTrackStates();
  }

  // virtual lists: <div data-vlist="key" data-row="track-row" data-row-height="56">
  const vlistData = new Map();
  function setList(key, items, extra) {
    vlistData.set(key, { items, extra: extra || {} });
  }
  function mountVirtualLists(root) {
    $$("[data-vlist]", root).forEach((el) => {
      const d = vlistData.get(el.dataset.vlist);
      if (!d) return;
      const rowH = Number(el.dataset.rowHeight) || 56;
      const comp = el.dataset.row || "track-row";
      const scroller = el.closest(".mp3-scroll, [data-region='main']") || region("main");
      el.style.position = "relative";
      el.style.height = d.items.length * rowH + "px";
      let lastRange = "";
      const draw = () => {
        if (!el.isConnected) return scroller.removeEventListener("scroll", onScroll);
        const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
        const viewTop = -top;
        const first = Math.max(0, Math.floor(viewTop / rowH) - 10);
        const last = Math.min(d.items.length, Math.ceil((viewTop + scroller.clientHeight) / rowH) + 10);
        const range = first + ":" + last;
        if (range === lastRange) return;
        lastRange = range;
        const out = [];
        for (let i = first; i < last; i++) {
          const item = d.items[i];
          out.push(
            `<div class="mp3-vrow" style="position:absolute;left:0;right:0;top:${i * rowH}px;height:${rowH}px" data-index="${i}">` +
              render(comp, Object.assign({ index: i, number: i + 1 }, d.extra, { track: item, item })) +
              "</div>"
          );
        }
        el.innerHTML = out.join("");
        updateTrackStates(el);
      };
      let raf = 0;
      const onScroll = () => {
        if (!raf) raf = requestAnimationFrame(() => ((raf = 0), draw()));
      };
      scroller.addEventListener("scroll", onScroll, { passive: true });
      requestAnimationFrame(draw);
      el._redraw = () => ((lastRange = ""), draw());
    });
  }
  function updateTrackStates(root = document) {
    const cur = S.playback && S.playback.track && S.playback.track.uri;
    $$("[data-track-uri]", root).forEach((el) => {
      el.classList.toggle("mp3-playing", el.dataset.trackUri === cur);
      el.classList.toggle("mp3-liked", S.likedSet.has(el.dataset.trackUri));
    });
  }

  async function cachedThen(cmd, args, draw, onFresh) {
    // Bridge returns {data, stale}; when stale, a libraryChanged event follows.
    const res = await call(cmd, args);
    return draw(res);
  }

  VIEWS.home = async (r) => {
    const sections = await call("browse.home");
    const hide = new Set((S.settings && S.settings.home.hiddenSections) || []);
    const showPod = !S.settings || S.settings.home.showPodcasts;
    const showBook = S.settings && S.settings.home.showAudiobooks;
    const visible = (sections || []).filter(
      (s) => !hide.has(s.id) && (s.kind !== "podcasts" || showPod) && (s.kind !== "audiobooks" || showBook)
    );
    const hour = new Date().getHours();
    const greeting = hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
    return render("view-home", { greeting, sections: visible, empty: !visible.length });
  };
  VIEWS.search = async (r, progressive) => {
    if (!r.query) return render("view-search", { query: "", results: null });
    progressive(render("view-search", { query: r.query, loading: true }));
    const res = await call("browse.search", { query: r.query, limit: 20 });
    setList("search-tracks", res.tracks.slice(0, 50));
    return render("view-search", { query: r.query, results: res, topTracks: res.tracks.slice(0, 4) });
  };
  VIEWS.library = async (r) => {
    const tab = r.query || "playlists";
    const [playlists, albums, artists, shows] = await Promise.all([
      call("library.playlists"),
      call("library.albums").catch(() => []),
      call("library.artists").catch(() => []),
      call("library.shows").catch(() => []),
    ]);
    return render("view-library", {
      tab,
      tabPlaylists: tab === "playlists",
      tabAlbums: tab === "albums",
      tabArtists: tab === "artists",
      tabShows: tab === "shows",
      playlists,
      albums,
      artists,
      shows,
    });
  };
  VIEWS.liked = async (r) => {
    const tracks = await call("library.liked");
    setList("liked", tracks, { contextUri: "spotify:collection:tracks" });
    return render("view-tracklist", {
      kind: "playlist",
      title: "Liked Songs",
      subtitle: `${tracks.length.toLocaleString()} songs`,
      uri: "spotify:collection:tracks",
      liked: true,
      listKey: "liked",
      count: tracks.length,
      showAdded: true,
      showAlbum: true,
    });
  };
  VIEWS.playlist = async (r) => {
    const pl = await call("browse.playlist", { uri: r.uri });
    setList("pl:" + r.uri, pl.tracks, { contextUri: r.uri, ownedByMe: pl.ownedByMe, playlistUri: r.uri });
    const total = pl.tracks.reduce((a, t) => a + (t.durationMs || 0), 0);
    return render("view-tracklist", {
      kind: "playlist",
      title: pl.name,
      subtitle: `${pl.owner} · ${pl.tracks.length.toLocaleString()} songs · ${fmtTime(total)}`,
      description: pl.description,
      images: pl.images,
      uri: r.uri,
      ownedByMe: pl.ownedByMe,
      listKey: "pl:" + r.uri,
      count: pl.tracks.length,
      showAdded: true,
      showAlbum: true,
    });
  };
  VIEWS.album = async (r) => {
    const al = await call("browse.album", { uri: r.uri });
    setList("al:" + r.uri, al.tracks, { contextUri: r.uri, hideCover: true });
    return render("view-tracklist", {
      kind: al.albumType || "album",
      title: al.name,
      artists: al.artists,
      subtitle: [al.releaseYear, `${al.tracks.length} songs`, fmtTime(al.tracks.reduce((a, t) => a + t.durationMs, 0))].filter(Boolean).join(" · "),
      images: al.images,
      uri: r.uri,
      isAlbum: true,
      label: al.label,
      listKey: "al:" + r.uri,
      count: al.tracks.length,
      showAlbum: false,
    });
  };
  VIEWS.artist = async (r) => {
    const ar = await call("browse.artist", { uri: r.uri });
    setList("ar:" + r.uri, ar.topTracks.slice(0, 10), { contextUri: r.uri });
    return render("view-artist", Object.assign({}, ar, { listKey: "ar:" + r.uri, topCount: Math.min(10, ar.topTracks.length) }));
  };
  VIEWS.show = async (r) => {
    const sh = await call("browse.show", { uri: r.uri });
    setList("sh:" + r.uri, sh.episodes, { contextUri: r.uri, episodes: true });
    return render("view-tracklist", {
      kind: "podcast",
      title: sh.name,
      subtitle: sh.publisher,
      description: sh.description,
      images: sh.images,
      uri: r.uri,
      listKey: "sh:" + r.uri,
      count: sh.episodes.length,
      episodes: true,
    });
  };
  VIEWS.nowPlaying = async () => {
    const t = S.playback && S.playback.track;
    if (t && !S.lyrics) loadLyrics();
    return render("now-playing", Object.assign(playerData(), { lyrics: S.lyrics, bigCover: t ? pickImage(t.album.images, 640) : "" }));
  };
  VIEWS.queue = async () => render("queue-panel", { queue: S.queue, full: true });
  VIEWS.lyrics = async () => {
    if (!S.lyrics) await loadLyrics();
    return render("lyrics-view", { lyrics: S.lyrics, full: true, track: S.playback && S.playback.track });
  };
  VIEWS.settings = async () => {
    const [settings, eq, presets] = await Promise.all([call("settings.get"), call("eq.get"), call("eq.presets")]);
    S.settings = settings;
    const homeSections = (await call("browse.home").catch(() => [])) || [];
    const hidden = new Set(settings.home.hiddenSections);
    return render("view-settings", {
      s: settings,
      eq,
      eqBands: eq.bands.map((b, i) => ({ i, freq: b.freq >= 1000 ? b.freq / 1000 + "k" : b.freq, gain: b.gainDb })),
      presets,
      crossfadeSec: settings.audio.crossfadeMs / 1000,
      sections: homeSections.map((s) => ({ id: s.id, title: s.title, kind: s.kind, shown: !hidden.has(s.id) })),
      session: S.session,
      quality320: settings.audio.quality === "very_high",
    });
  };
  VIEWS.themes = async () => {
    const [themes, perf] = await Promise.all([call("themes.list"), call("themes.perf").catch(() => null)]);
    const active = S.settings ? S.settings.theme : "default";
    return render("view-themes", {
      themes: themes.map((t) => Object.assign(t, { active: t.id === active })),
      perf,
      liveLink: S.settings && S.settings.liveLink,
    });
  };
  VIEWS.editor = async (r) => {
    const id = r.query;
    const files = await call("themes.read", { id });
    let meta = {};
    try {
      meta = JSON.parse(files["theme.json"] || "{}");
    } catch {}
    editorState = { id, files, meta };
    const colors = Object.entries(meta.colors || {}).map(([k, v]) => ({ key: k, value: v }));
    return render("view-editor", {
      id,
      name: meta.name,
      colors,
      fonts: meta.fonts || {},
      radius: meta.radius ?? 8,
      density: meta.density || "comfortable",
      css: files["theme.css"] || "",
      layout: files["layout.json"] || "",
      readOnly: id === "default",
    });
  };
  let editorState = null;

  // ------------------------------------------------------------------ data refreshers
  async function refreshPlaylists() {
    try {
      S.playlists = await call("library.playlists");
      renderRegion("sidebar");
    } catch (e) {}
  }
  async function refreshLiked() {
    try {
      const liked = await call("library.liked");
      S.likedSet = new Set(liked.map((t) => t.uri));
      renderRegion("player");
      updateTrackStates();
    } catch (e) {}
  }
  async function loadLyrics() {
    const t = S.playback && S.playback.track;
    if (!t) {
      S.lyrics = null;
      return;
    }
    const uri = t.uri;
    try {
      const l = await call("browse.lyrics", { uri });
      if (S.playback && S.playback.track && S.playback.track.uri === uri) {
        S.lyrics = l || { lines: [], synced: false, none: true };
        if (S.panel === "lyrics") renderRegion("panel");
        if (S.route && (S.route.view === "nowPlaying" || S.route.view === "lyrics")) renderRoute();
      }
    } catch (e) {
      S.lyrics = { lines: [], none: true, error: e.message };
    }
  }
  let lastLyricIdx = -1;
  function syncLyrics(force) {
    const L = S.lyrics;
    if (!L || !L.synced || !L.lines.length) return;
    const pos = positionNow() + 150;
    let idx = -1;
    for (let i = 0; i < L.lines.length; i++) {
      if (L.lines[i].startMs <= pos) idx = i;
      else break;
    }
    if (idx === lastLyricIdx && !force) return;
    lastLyricIdx = idx;
    $$(".mp3-lyrics").forEach((box) => {
      $$(".mp3-lyric-line", box).forEach((el, i) => {
        el.classList.toggle("mp3-lyric-active", i === idx);
        el.classList.toggle("mp3-lyric-past", i < idx);
      });
      const active = box.querySelector(".mp3-lyric-active");
      if (active) {
        const top = active.offsetTop - box.clientHeight / 2 + active.clientHeight / 2;
        box.scrollTo({ top, behavior: force ? "auto" : "smooth" });
      }
    });
  }

  // ------------------------------------------------------------------ actions
  const ACTIONS = {};
  function rowContext(el) {
    const row = el.closest("[data-index]");
    const list = el.closest("[data-vlist]");
    if (!row || !list) return null;
    const d = vlistData.get(list.dataset.vlist);
    if (!d) return null;
    const i = Number(row.dataset.index);
    return { index: i, track: d.items[i], extra: d.extra, list: d };
  }
  ACTIONS.play = (el) => {
    const uri = el.dataset.uri;
    const ctx = el.dataset.context;
    const rc = rowContext(el);
    if (rc && rc.extra.contextUri) {
      return call("player.play", { uri: rc.extra.contextUri, trackUri: rc.track.uri, index: rc.index });
    }
    if (ctx) return call("player.play", { uri: ctx, trackUri: uri || undefined });
    if (uri) return call("player.play", { uri });
  };
  ACTIONS["play-context"] = (el) => call("player.play", { uri: el.dataset.uri });
  ACTIONS["shuffle-context"] = async (el) => {
    await call("player.setShuffle", { mode: "on" });
    return call("player.play", { uri: el.dataset.uri });
  };
  ACTIONS.toggle = () => call("player.toggle");
  ACTIONS.next = () => call("player.next");
  ACTIONS.prev = () => call("player.prev");
  ACTIONS.shuffle = () => {
    const cur = (S.playback && S.playback.shuffle) || "off";
    const next = cur === "off" ? "on" : cur === "on" ? "spread" : "off";
    return call("player.setShuffle", { mode: next }).then(() => toast(next === "spread" ? "Shuffle: spread out artists" : next === "on" ? "Shuffle on" : "Shuffle off"));
  };
  ACTIONS.repeat = () => {
    const cur = (S.playback && S.playback.repeat) || "off";
    const next = cur === "off" ? "all" : cur === "all" ? "one" : "off";
    return call("player.setRepeat", { mode: next });
  };
  ACTIONS.like = async (el) => {
    const uri = el.dataset.uri || (rowContext(el) || {}).track?.uri || (S.playback && S.playback.track && S.playback.track.uri);
    if (!uri) return;
    const liked = S.likedSet.has(uri);
    if (liked) S.likedSet.delete(uri);
    else S.likedSet.add(uri);
    renderRegion("player");
    updateTrackStates();
    try {
      await call(liked ? "library.unlike" : "library.like", { uris: [uri] });
      toast(liked ? "Removed from Liked Songs" : "Added to Liked Songs");
    } catch (e) {
      if (liked) S.likedSet.add(uri);
      else S.likedSet.delete(uri);
      renderRegion("player");
      throw e;
    }
  };
  ACTIONS.open = (el) => {
    const uri = el.dataset.uri;
    const k = kindOf(uri);
    if (uri === "spotify:collection:tracks" || uri.endsWith(":collection")) return navigate("liked");
    if (["playlist", "album", "artist", "show"].includes(k)) return navigate(k, { uri });
    if (k === "track" || k === "episode") return call("player.play", { uri });
  };
  ACTIONS.nav = (el) => navigate(el.dataset.view, { query: el.dataset.query || undefined });
  ACTIONS.back = () => back();
  ACTIONS.forward = () => forward();
  ACTIONS["now-playing"] = () => (S.route && S.route.view === "nowPlaying" ? back() : navigate("nowPlaying"));
  ACTIONS["toggle-panel"] = (el) => {
    const p = el.dataset.panel || "queue";
    S.panel = S.panel === p ? "none" : p;
    if (S.panel === "lyrics" && !S.lyrics) loadLyrics();
    call("settings.set", { patch: { ui: { rightPanel: S.panel } } }).catch(() => {});
    applyLayout();
  };
  ACTIONS["toggle-sidebar"] = () => {
    S.sidebarCollapsed = !S.sidebarCollapsed;
    call("settings.set", { patch: { ui: { sidebarCollapsed: S.sidebarCollapsed } } }).catch(() => {});
    applyLayout();
  };
  ACTIONS["toggle-compact"] = () => setCompact(!S.compact);
  ACTIONS["queue-add"] = (el) => queueUris(el, "queue.add", "Added to queue");
  ACTIONS["play-next"] = (el) => queueUris(el, "queue.playNext", "Playing next");
  ACTIONS["queue-remove"] = (el) => call("queue.remove", { uid: Number(el.dataset.uid) });
  ACTIONS["queue-clear"] = () => call("queue.clear");
  ACTIONS["queue-play"] = (el) => call("player.play", { uri: el.dataset.uri });
  function queueUris(el, cmd, msg) {
    const uri = el.dataset.uri || (rowContext(el) || {}).track?.uri;
    if (!uri) return;
    return call(cmd, { uris: [uri] }).then(() => toast(msg));
  }
  ACTIONS.seek = (el, ev) => {
    const dur = (S.playback && S.playback.durationMs) || 0;
    if (!dur) return;
    let ratio;
    if (el.tagName === "INPUT") ratio = Number(el.value) / Number(el.max || 1000);
    else {
      const r = el.getBoundingClientRect();
      ratio = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
    }
    return call("player.seek", { positionMs: Math.round(ratio * dur) });
  };
  ACTIONS.volume = (el, ev) => {
    let v;
    if (el.tagName === "INPUT") v = Number(el.value) / Number(el.max || 100);
    else {
      const r = el.getBoundingClientRect();
      v = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
    }
    if (S.playback) S.playback.volume = v;
    return call("player.setVolume", { volume: v });
  };
  ACTIONS.mute = () => {
    const v = (S.playback && S.playback.volume) || 0;
    if (v > 0) {
      S.lastVolume = v;
      return call("player.setVolume", { volume: 0 });
    }
    return call("player.setVolume", { volume: S.lastVolume || 0.8 });
  };
  ACTIONS.search = (el) => searchDebounced(el.value);
  const searchDebounced = debounce((q) => {
    q = q.trim();
    if (!q) return;
    navigate("search", { query: q }, { replace: S.route && S.route.view === "search" });
  }, 300);
  ACTIONS["library-tab"] = (el) => navigate("library", { query: el.dataset.tab }, { replace: true });
  ACTIONS["create-playlist"] = async () => {
    const name = await promptText("New playlist", "My playlist");
    if (!name) return;
    const uri = await call("playlist.create", { name });
    await refreshPlaylists();
    navigate("playlist", { uri });
  };
  ACTIONS["rename-playlist"] = async (el) => {
    const uri = el.dataset.uri;
    const name = await promptText("Rename playlist", el.dataset.name || "");
    if (!name) return;
    await call("playlist.rename", { uri, name });
    await refreshPlaylists();
    renderRoute();
  };
  ACTIONS["delete-playlist"] = async (el) => {
    await call("playlist.delete", { uri: el.dataset.uri });
    await refreshPlaylists();
    navigate("library");
  };
  ACTIONS["remove-from-playlist"] = async (el) => {
    const rc = rowContext(el);
    if (!rc || !rc.extra.playlistUri) return;
    await call("playlist.removeTracks", { uri: rc.extra.playlistUri, indices: [rc.index] });
    toast("Removed from playlist");
    renderRoute();
  };
  ACTIONS["add-to-playlist"] = async (el) => {
    const uri = el.dataset.uri || (rowContext(el) || {}).track?.uri;
    await call("playlist.addTracks", { uri: el.dataset.playlist, uris: [uri] });
    toast("Added to playlist");
  };
  ACTIONS["save-album"] = async (el) => {
    await call("library.saveAlbum", { uri: el.dataset.uri });
    toast("Saved to your library");
  };
  ACTIONS.retry = () => renderRoute();
  ACTIONS.login = () => call("session.login");
  ACTIONS.logout = () => call("session.logout");
  ACTIONS.window = (el) => call("window.action", { action: el.dataset.window });
  ACTIONS["command-bar"] = () => openCommandBar();
  ACTIONS["context-menu"] = (el, ev) => openTrackMenu(el, ev);
  ACTIONS["home-hide"] = async (el) => {
    const id = el.dataset.id;
    const hidden = new Set(S.settings.home.hiddenSections);
    hidden.add(id);
    S.settings = await call("settings.set", { patch: { home: { hiddenSections: [...hidden] } } });
    renderRoute();
  };
  // settings inputs: <input data-setting="audio.crossfadeMs" data-scale="1000">
  ACTIONS.setting = async (el) => {
    const path = el.dataset.setting.split(".");
    let v = el.type === "checkbox" ? el.checked : el.value;
    if (el.type === "range" || el.type === "number") v = Number(v) * Number(el.dataset.scale || 1);
    if (el.dataset.invert) v = !v;
    const patch = {};
    let cur = patch;
    path.forEach((p, i) => (cur = cur[p] = i === path.length - 1 ? v : {}));
    try {
      S.settings = await call("settings.set", { patch });
    } catch (e) {
      toast(e.message, "error");
    }
  };
  ACTIONS["home-section"] = async (el) => {
    const hidden = new Set(S.settings.home.hiddenSections);
    if (el.checked) hidden.delete(el.dataset.id);
    else hidden.add(el.dataset.id);
    S.settings = await call("settings.set", { patch: { home: { hiddenSections: [...hidden] } } });
  };
  ACTIONS["eq-band"] = debounce(async (el) => {
    const gains = $$("[data-action='eq-band']").map((b) => Number(b.value));
    await call("eq.set", { gains, enabled: true });
  }, 60);
  ACTIONS["eq-toggle"] = (el) => call("eq.set", { enabled: el.checked });
  ACTIONS["eq-preset"] = async (el) => {
    await call("eq.applyPreset", { name: el.value || el.dataset.name });
    if (S.route.view === "settings") renderRoute();
  };
  ACTIONS["eq-save"] = async () => {
    const name = await promptText("Save EQ preset", "My preset");
    if (name) {
      await call("eq.savePreset", { name });
      renderRoute();
    }
  };
  // themes
  ACTIONS["theme-apply"] = (el) => call("themes.apply", { id: el.dataset.id });
  ACTIONS["theme-import"] = async () => {
    await call("themes.import");
    renderRoute();
  };
  ACTIONS["theme-duplicate"] = async (el) => {
    await call("themes.duplicate", { id: el.dataset.id });
    toast("Theme duplicated");
    renderRoute();
  };
  ACTIONS["theme-export"] = (el) => call("themes.export", { id: el.dataset.id });
  ACTIONS["theme-delete"] = async (el) => {
    await call("themes.delete", { id: el.dataset.id });
    renderRoute();
  };
  ACTIONS["theme-edit"] = (el) => navigate("editor", { query: el.dataset.id });
  ACTIONS["theme-guide"] = async () => {
    await call("themes.guide");
    toast("Theme guide copied to clipboard");
  };
  ACTIONS["theme-live-link"] = async () => {
    await call("themes.liveLink");
    renderRoute();
  };
  ACTIONS["theme-live-unlink"] = async () => {
    await call("themes.liveUnlink");
    renderRoute();
  };
  // editor
  function editorCollect() {
    const meta = Object.assign({}, editorState.meta);
    meta.colors = Object.assign({}, meta.colors);
    $$("[data-editor-color]").forEach((i) => (meta.colors[i.dataset.editorColor] = i.value));
    meta.fonts = Object.assign({}, meta.fonts);
    $$("[data-editor-font]").forEach((i) => (meta.fonts[i.dataset.editorFont] = i.value));
    const r = $("[data-editor='radius']");
    if (r) meta.radius = Number(r.value);
    const d = $("[data-editor='density']");
    if (d) meta.density = d.value;
    const files = { "theme.json": JSON.stringify(meta, null, 2) };
    const css = $("[data-editor='css']");
    if (css) files["theme.css"] = css.value;
    const layout = $("[data-editor='layout']");
    if (layout && layout.value.trim()) files["layout.json"] = layout.value;
    return { meta, files };
  }
  ACTIONS["editor-preview"] = debounce(() => {
    if (!editorState) return;
    const { meta, files } = editorCollect();
    // Live preview inside this frame: tokens + css only.
    const root = document.documentElement.style;
    const map = { bg: "--bg", surface: "--surface", surface2: "--surface-2", elevated: "--elevated", text: "--text", textMuted: "--text-muted", accent: "--accent", accentText: "--accent-text", border: "--border", hover: "--hover", danger: "--danger" };
    for (const [k, v] of Object.entries(meta.colors || {})) if (map[k]) root.setProperty(map[k], v);
    if (meta.radius != null) root.setProperty("--radius", meta.radius + "px");
    let st = $("#mp3-editor-preview");
    if (!st) {
      st = document.createElement("style");
      st.id = "mp3-editor-preview";
      document.head.appendChild(st);
    }
    st.textContent = files["theme.css"] || "";
  }, 150);
  ACTIONS["editor-save"] = async () => {
    const { files } = editorCollect();
    try {
      await call("themes.save", { id: editorState.id, files });
      toast("Theme saved");
    } catch (e) {
      toast(e.message, "error");
    }
  };
  ACTIONS["editor-export"] = () => call("themes.export", { id: editorState.id });
  ACTIONS["close-overlay"] = () => closeOverlay();

  function runAction(el, ev) {
    const name = el.dataset.action;
    const fn = ACTIONS[name];
    if (!fn) return console.warn("unknown action", name);
    Promise.resolve()
      .then(() => fn(el, ev))
      .catch((e) => toast(e.message || String(e), "error"));
  }
  document.addEventListener("click", (ev) => {
    const el = ev.target.closest("[data-action]");
    if (!el || el.tagName === "INPUT" || el.tagName === "SELECT" || el.tagName === "TEXTAREA") return;
    if (el.dataset.trigger && el.dataset.trigger !== "click") return;
    ev.preventDefault();
    runAction(el, ev);
  });
  document.addEventListener("dblclick", (ev) => {
    const el = ev.target.closest("[data-dblaction]");
    if (!el) return;
    ev.preventDefault();
    const a = el.dataset.action;
    el.dataset.action = el.dataset.dblaction;
    runAction(el, ev);
    el.dataset.action = a;
  });
  document.addEventListener("input", (ev) => {
    const el = ev.target.closest("[data-action]");
    if (!el) return;
    if (["seek"].includes(el.dataset.action) && ev.type === "input") return; // commit on change
    runAction(el, ev);
  });
  document.addEventListener("change", (ev) => {
    const el = ev.target.closest("[data-action]");
    if (el && el.dataset.action === "seek") runAction(el, ev);
  });
  document.addEventListener("contextmenu", (ev) => {
    const row = ev.target.closest("[data-track-uri]");
    if (!row) return;
    ev.preventDefault();
    openTrackMenu(row, ev);
  });

  // ------------------------------------------------------------------ overlays
  function closeOverlay() {
    $$(".mp3-overlay").forEach((o) => o.remove());
  }
  function openOverlay(html, cls) {
    closeOverlay();
    const o = document.createElement("div");
    o.className = "mp3-overlay " + (cls || "");
    o.innerHTML = html;
    o.addEventListener("mousedown", (e) => {
      if (e.target === o) closeOverlay();
    });
    document.body.appendChild(o);
    return o;
  }
  function promptText(title, value) {
    return new Promise((resolve) => {
      const o = openOverlay(render("prompt-dialog", { title, value }), "mp3-overlay-dim");
      const input = $("input", o);
      input.focus();
      input.select();
      const done = (v) => {
        closeOverlay();
        resolve(v);
      };
      o.addEventListener("keydown", (e) => {
        if (e.key === "Enter") done(input.value.trim());
        if (e.key === "Escape") done(null);
      });
      $$("[data-prompt]", o).forEach((b) => b.addEventListener("click", () => done(b.dataset.prompt === "ok" ? input.value.trim() : null)));
    });
  }
  function openTrackMenu(el, ev) {
    const rc = rowContext(el);
    const uri = el.dataset.trackUri || el.dataset.uri || (rc && rc.track && rc.track.uri);
    const track = (rc && rc.track) || (S.playback && S.playback.track && S.playback.track.uri === uri ? S.playback.track : null);
    if (!uri) return;
    const data = {
      uri,
      track,
      liked: S.likedSet.has(uri),
      playlists: S.playlists.filter((p) => p.ownedByMe || p.collaborative),
      canRemove: rc && rc.extra.ownedByMe,
      album: track && track.album,
      artists: track && track.artists,
    };
    const o = openOverlay(render("context-menu", data), "mp3-overlay-menu");
    const menu = o.firstElementChild;
    const x = Math.min(ev.clientX, innerWidth - menu.offsetWidth - 8);
    const y = Math.min(ev.clientY, innerHeight - menu.offsetHeight - 8);
    menu.style.left = x + "px";
    menu.style.top = y + "px";
    // proxy row context for actions inside the menu
    menu.addEventListener("click", (e) => {
      const a = e.target.closest("[data-action]");
      if (!a) return;
      e.stopPropagation();
      e.preventDefault();
      if (!a.dataset.uri) a.dataset.uri = uri;
      if (a.dataset.action === "remove-from-playlist" && rc) {
        call("playlist.removeTracks", { uri: rc.extra.playlistUri, indices: [rc.index] }).then(() => {
          toast("Removed from playlist");
          renderRoute();
        });
      } else runAction(a, e);
      closeOverlay();
    });
  }

  // Ctrl+K command bar
  const COMMANDS = [
    { title: "Play / pause", hint: "Space", run: () => call("player.toggle") },
    { title: "Next track", hint: "Ctrl+→", run: () => call("player.next") },
    { title: "Previous track", hint: "Ctrl+←", run: () => call("player.prev") },
    { title: "Like current song", hint: "Ctrl+L", run: () => ACTIONS.like({ dataset: {} }) },
    { title: "Toggle shuffle", run: () => ACTIONS.shuffle() },
    { title: "Toggle repeat", run: () => ACTIONS.repeat() },
    { title: "Go to Home", run: () => navigate("home") },
    { title: "Go to Search", hint: "Ctrl+F", run: () => navigate("search") },
    { title: "Go to Library", run: () => navigate("library") },
    { title: "Liked Songs", run: () => navigate("liked") },
    { title: "Now Playing", run: () => navigate("nowPlaying") },
    { title: "Show queue", run: () => ACTIONS["toggle-panel"]({ dataset: { panel: "queue" } }) },
    { title: "Show lyrics", run: () => ACTIONS["toggle-panel"]({ dataset: { panel: "lyrics" } }) },
    { title: "Settings", run: () => navigate("settings") },
    { title: "Themes", run: () => navigate("themes") },
    { title: "Toggle compact mode", run: () => setCompact(!S.compact) },
    { title: "Mini player", run: () => call("window.action", { action: "mini" }) },
    { title: "Create playlist", run: () => ACTIONS["create-playlist"]() },
    { title: "Clear queue", run: () => call("queue.clear") },
    { title: "Reset theme to Default", hint: "Ctrl+Shift+D", run: () => call("themes.apply", { id: "default" }) },
  ];
  function openCommandBar() {
    const o = openOverlay(render("command-bar", {}), "mp3-overlay-dim mp3-overlay-top");
    const input = $("input", o);
    const list = $(".mp3-cmd-list", o);
    let items = [];
    let sel = 0;
    const draw = () => {
      list.innerHTML = items
        .map((it, i) => `<div class="mp3-cmd-item${i === sel ? " mp3-selected" : ""}" data-i="${i}"><span>${esc(it.title)}</span><small>${esc(it.hint || it.kind || "")}</small></div>`)
        .join("");
      const s = list.children[sel];
      if (s) s.scrollIntoView({ block: "nearest" });
    };
    const fuzzy = (q, t) => {
      q = q.toLowerCase();
      t = t.toLowerCase();
      let j = 0;
      for (const c of t) if (c === q[j]) j++;
      return j === q.length;
    };
    const update = debounce(async () => {
      const q = input.value.trim();
      const pl = S.playlists.map((p) => ({ title: p.name, kind: "Playlist", run: () => navigate("playlist", { uri: p.uri }) }));
      items = [...COMMANDS, ...pl].filter((c) => !q || fuzzy(q, c.title)).slice(0, 12);
      sel = 0;
      draw();
      if (q.length >= 2) {
        try {
          const r = await call("browse.search", { query: q, limit: 5 });
          if (input.value.trim() !== q) return;
          const extra = [
            ...r.tracks.slice(0, 5).map((t) => ({ title: `${t.title} — ${t.artists.map((a) => a.name).join(", ")}`, kind: "Song", run: () => call("player.play", { uri: t.uri }) })),
            ...r.artists.slice(0, 2).map((a) => ({ title: a.name, kind: "Artist", run: () => navigate("artist", { uri: a.uri }) })),
            ...r.albums.slice(0, 2).map((a) => ({ title: a.name, kind: "Album", run: () => navigate("album", { uri: a.uri }) })),
            { title: `Search for “${q}”`, kind: "Search", run: () => navigate("search", { query: q }) },
          ];
          items = items.concat(extra).slice(0, 20);
          draw();
        } catch (e) {}
      }
    }, 120);
    input.addEventListener("input", update);
    input.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") (sel = Math.min(items.length - 1, sel + 1)), draw(), e.preventDefault();
      else if (e.key === "ArrowUp") (sel = Math.max(0, sel - 1)), draw(), e.preventDefault();
      else if (e.key === "Enter" && items[sel]) {
        closeOverlay();
        Promise.resolve(items[sel].run()).catch((err) => toast(err.message, "error"));
      } else if (e.key === "Escape") closeOverlay();
    });
    list.addEventListener("click", (e) => {
      const it = e.target.closest("[data-i]");
      if (!it) return;
      closeOverlay();
      Promise.resolve(items[Number(it.dataset.i)].run()).catch((err) => toast(err.message, "error"));
    });
    update();
    input.focus();
  }

  // ------------------------------------------------------------------ keyboard
  document.addEventListener("keydown", (e) => {
    const typing = e.target.matches("input[type=text], input[type=search], input:not([type]), textarea, [contenteditable]");
    const k = e.key;
    if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === "k") return e.preventDefault(), openCommandBar();
    if (k === "Escape") return closeOverlay();
    if (typing) return;
    if (k === " ") return e.preventDefault(), call("player.toggle");
    if ((e.ctrlKey || e.metaKey) && k === "ArrowRight") return e.preventDefault(), call("player.next");
    if ((e.ctrlKey || e.metaKey) && k === "ArrowLeft") return e.preventDefault(), call("player.prev");
    if (e.altKey && k === "ArrowLeft") return e.preventDefault(), back();
    if (e.altKey && k === "ArrowRight") return e.preventDefault(), forward();
    if (k === "ArrowRight") return call("player.seek", { positionMs: positionNow() + 5000 });
    if (k === "ArrowLeft") return call("player.seek", { positionMs: Math.max(0, positionNow() - 5000) });
    if (k === "ArrowUp" && (e.ctrlKey || e.shiftKey)) return e.preventDefault(), call("player.setVolume", { volume: Math.min(1, ((S.playback && S.playback.volume) || 0) + 0.05) });
    if (k === "ArrowDown" && (e.ctrlKey || e.shiftKey)) return e.preventDefault(), call("player.setVolume", { volume: Math.max(0, ((S.playback && S.playback.volume) || 0) - 0.05) });
    if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === "l") return e.preventDefault(), ACTIONS.like({ dataset: {} });
    if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === "f") {
      e.preventDefault();
      navigate("search");
      setTimeout(() => $("[data-action='search']") && $("[data-action='search']").focus(), 50);
      return;
    }
    if (k.toLowerCase() === "s" && !e.ctrlKey) return ACTIONS.shuffle();
    if (k.toLowerCase() === "r" && !e.ctrlKey) return ACTIONS.repeat();
    if (k.toLowerCase() === "m" && !e.ctrlKey) return ACTIONS.mute();
    if (k.toLowerCase() === "q" && !e.ctrlKey) return ACTIONS["toggle-panel"]({ dataset: { panel: "queue" } });
    if (k.toLowerCase() === "y" && !e.ctrlKey) return ACTIONS["toggle-panel"]({ dataset: { panel: "lyrics" } });
  });

  function setCompact(on) {
    S.compact = on;
    call("settings.set", { patch: { ui: { compact: on } } }).catch(() => {});
    applyLayout();
  }

  // ------------------------------------------------------------------ events from engine
  function setAccent() {
    const t = S.playback && S.playback.track;
    const fromCover = BOOT.theme.meta.accentFromCover !== false;
    const rs = document.documentElement.style;
    if (fromCover && S.playback && S.playback.accent) rs.setProperty("--accent", S.playback.accent);
    else rs.removeProperty("--accent");
    const cover = t ? pickImage(t.album && t.album.images, 640) : "";
    if (cover) rs.setProperty("--np-cover", `url("${cover}")`);
    else rs.removeProperty("--np-cover");
  }
  on("trackChanged", (st) => {
    const prev = S.playback && S.playback.track && S.playback.track.uri;
    S.playback = st;
    if (!st.track || st.track.uri !== prev) {
      S.lyrics = null;
      lastLyricIdx = -1;
      if (S.panel === "lyrics" || (S.route && ["nowPlaying", "lyrics"].includes(S.route.view))) loadLyrics();
    }
    setAccent();
    renderRegion("player");
    if (S.panel === "lyrics") renderRegion("panel");
    if (S.route && S.route.view === "nowPlaying") renderRoute();
    updateTrackStates();
  });
  on("playStateChanged", (st) => {
    const accent = S.playback && S.playback.accent;
    S.playback = Object.assign({ accent }, st, st.accent ? { accent: st.accent } : {});
    renderRegion("player");
  });
  on("progress", (p) => {
    if (S.playback) {
      S.playback.positionMs = p.positionMs;
      S.playback.positionAt = Date.now();
      if (p.durationMs) S.playback.durationMs = p.durationMs;
    }
  });
  on("queueChanged", (q) => {
    S.queue = q;
    if (S.panel === "queue") renderRegion("panel");
    if (S.route && S.route.view === "queue") renderRoute();
  });
  on("libraryChanged", (d) => {
    const key = d && d.key;
    if (key === "lib:playlists") refreshPlaylists();
    if (key === "lib:liked") refreshLiked();
    const r = S.route;
    if (!r) return;
    const viewKey = { home: "home", liked: "lib:liked", library: "lib:playlists" }[r.view] || (r.uri ? { playlist: "pl:", album: "al:", artist: "ar:", show: "sh:" }[r.view] + r.uri : null);
    if (viewKey && key === viewKey) renderRoute();
  });
  on("sessionChanged", (s) => {
    S.session = s;
    if (s.state === "connected") {
      refreshPlaylists();
      refreshLiked();
    }
    renderRegion("sidebar");
    if (s.state === "loggedOut" || s.state === "error") renderRoute();
  });
  on("settingsChanged", (s) => {
    S.settings = s;
  });
  on("navigate", (r) => {
    if (r && r.external) navigate(r.view, r);
  });
  on("error", (e) => toast(e.message || String(e), "error"));

  function on(event, cb) {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event).add(cb);
    return () => listeners.get(event).delete(cb);
  }

  // ------------------------------------------------------------------ public SDK
  const mp3 = {
    apiVersion: API_VERSION,
    theme: BOOT.theme,
    call,
    on,
    off: (event, cb) => listeners.get(event) && listeners.get(event).delete(cb),
    player: {
      play: (uri, opts = {}) => call("player.play", Object.assign({ uri }, opts)),
      resume: () => call("player.resume"),
      pause: () => call("player.pause"),
      toggle: () => call("player.toggle"),
      next: () => call("player.next"),
      prev: () => call("player.prev"),
      seek: (ms) => call("player.seek", { positionMs: Math.max(0, Math.round(ms)) }),
      setVolume: (v) => call("player.setVolume", { volume: v }),
      setShuffle: (mode) => call("player.setShuffle", { mode: mode === true ? "on" : mode === false ? "off" : mode }),
      setRepeat: (mode) => call("player.setRepeat", { mode }),
      state: () => call("player.state"),
    },
    get nowPlaying() {
      const p = S.playback || {};
      const t = p.track || null;
      return {
        track: t,
        title: t && t.title,
        artists: t ? t.artists : [],
        album: t ? t.album : null,
        coverUrl: t ? pickImage(t.album && t.album.images, 640) : "",
        accent: p.accent || null,
        playing: !!p.playing,
        positionMs: positionNow(),
        durationMs: p.durationMs || 0,
        volume: p.volume,
        shuffle: p.shuffle,
        repeat: p.repeat,
      };
    },
    library: {
      playlists: () => call("library.playlists"),
      liked: () => call("library.liked"),
      albums: () => call("library.albums"),
      artists: () => call("library.artists"),
      shows: () => call("library.shows"),
      playlist: (uri) => call("browse.playlist", { uri }),
      album: (uri) => call("browse.album", { uri }),
      artist: (uri) => call("browse.artist", { uri }),
      show: (uri) => call("browse.show", { uri }),
      search: (query, limit) => call("browse.search", { query, limit }),
      home: () => call("browse.home"),
      recommendations: (uris) => call("browse.recommendations", { uris }),
      lyrics: (uri) => call("browse.lyrics", { uri }),
      isLiked: (uris) => call("library.isLiked", { uris }),
      like: (uris) => call("library.like", { uris }),
      unlike: (uris) => call("library.unlike", { uris }),
    },
    queue: {
      get: () => call("queue.get"),
      add: (uris) => call("queue.add", { uris: [].concat(uris) }),
      playNext: (uris) => call("queue.playNext", { uris: [].concat(uris) }),
      remove: (uid) => call("queue.remove", { uid }),
      move: (uid, to) => call("queue.move", { uid, to }),
      clear: () => call("queue.clear"),
    },
    audio: {
      bins: 64,
      subscribe(cb) {
        audioSubs.add(cb);
        if (audioSubs.size === 1) call("audio.subscribe", { enabled: true }).catch(() => {});
        return () => {
          audioSubs.delete(cb);
          if (audioSubs.size === 0) call("audio.subscribe", { enabled: false }).catch(() => {});
        };
      },
    },
    eq: {
      get: () => call("eq.get"),
      set: (s) => call("eq.set", s),
      presets: () => call("eq.presets"),
      applyPreset: (name) => call("eq.applyPreset", { name }),
      savePreset: (name) => call("eq.savePreset", { name }),
      deletePreset: (name) => call("eq.deletePreset", { name }),
    },
    nav: {
      open: (view, params = {}) => navigate(view, params),
      back,
      forward,
      get current() {
        return S.route;
      },
    },
    storage: {
      get: (key) => call("storage.get", { key }),
      set: (key, value) => call("storage.set", { key, value }),
      remove: (key) => call("storage.remove", { key }),
      keys: () => call("storage.keys"),
    },
    ui: {
      render,
      rerender: () => (applyLayout(), renderRoute()),
      registerAction: (name, fn) => (ACTIONS[name] = fn),
      registerView: (name, fn) => (VIEWS[name] = fn),
      registerFilter: (name, fn) => (FILTERS[name] = fn),
      registerComponent: (name, src) => {
        BOOT.components[name] = String(src);
        tplCache.delete(name);
      },
      component: (name) => (BOOT.components || {})[name],
      registerCommand: (cmd) => COMMANDS.push(cmd),
      setList,
      toast,
      formatTime: fmtTime,
      pickImage,
      region,
    },
  };
  Object.defineProperty(window, "mp3", { value: Object.freeze(mp3), writable: false });

  // ------------------------------------------------------------------ boot
  async function boot() {
    // All in parallel; any one failing (offline, audio still starting) must not block the UI.
    const [session, settings, playback, queue, playlists] = await Promise.allSettled([
      call("session.state"),
      call("settings.get"),
      call("player.state"),
      call("queue.get"),
      call("library.playlists"),
    ]);
    if (session.status === "fulfilled") S.session = session.value;
    if (settings.status === "fulfilled") {
      S.settings = settings.value;
      S.panel = S.settings.ui.rightPanel || "queue";
      S.sidebarCollapsed = !!S.settings.ui.sidebarCollapsed;
      S.compact = !!S.settings.ui.compact;
    }
    if (playback.status === "fulfilled") S.playback = playback.value;
    if (queue.status === "fulfilled") S.queue = queue.value;
    if (playlists.status === "fulfilled") S.playlists = playlists.value;
    S.route = { view: "home" };
    setAccent();
    applyLayout();
    post({ ready: true, apiVersion: API_VERSION });
    refreshLiked();
  }
  // Give theme scripts (loaded after this file) a chance to register views first.
  if (document.readyState === "complete") setTimeout(boot, 0);
  else window.addEventListener("load", () => setTimeout(boot, 0));
  post({ hello: true, apiVersion: API_VERSION });
})();
