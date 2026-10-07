// Dev helper: drive the running app's WebView2 over the Chrome DevTools
// Protocol (start the app with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=
// --remote-debugging-port=9222).
//
//   node scripts/cdp.mjs targets
//   node scripts/cdp.mjs shot out.png            screenshot of the main window
//   node scripts/cdp.mjs eval "<js>" [frame]      evaluate in host (or theme frame)
//   node scripts/cdp.mjs console 5                print console messages for N seconds
const PORT = process.env.CDP_PORT || 9222;
const [, , cmd, a1, a2] = process.argv;

async function targets() {
  const r = await fetch(`http://127.0.0.1:${PORT}/json/list`);
  return r.json();
}

function connect(wsUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    const handlers = [];
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pending.has(m.id)) {
        const { res, rej } = pending.get(m.id);
        pending.delete(m.id);
        m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result);
      } else handlers.forEach((h) => h(m));
    };
    ws.onopen = () =>
      resolve({
        send: (method, params = {}, sessionId) =>
          new Promise((res, rej) => {
            const mid = ++id;
            pending.set(mid, { res, rej });
            ws.send(JSON.stringify({ id: mid, method, params, ...(sessionId ? { sessionId } : {}) }));
          }),
        on: (h) => handlers.push(h),
        close: () => ws.close(),
      });
    ws.onerror = reject;
  });
}

async function page() {
  const ts = await targets();
  const p = ts.find((t) => t.type === "page" && /tauri\.localhost/.test(t.url));
  if (!p) throw new Error("no app page: " + JSON.stringify(ts.map((t) => [t.type, t.url])));
  return { target: p, all: ts };
}

async function evalIn(js, inFrame) {
  const { target, all } = await page();
  if (inFrame) {
    const f = all.find((t) => t.type === "iframe" && /themeb?\.localhost/.test(t.url));
    if (f) {
      const c = await connect(f.webSocketDebuggerUrl);
      const r = await c.send("Runtime.evaluate", { expression: js, awaitPromise: true, returnByValue: true });
      c.close();
      return r;
    }
    // Same-process frame: find its execution context.
    const c = await connect(target.webSocketDebuggerUrl);
    const ctxs = [];
    c.on((m) => m.method === "Runtime.executionContextCreated" && ctxs.push(m.params.context));
    await c.send("Runtime.enable");
    await new Promise((r) => setTimeout(r, 300));
    const ctx = ctxs.find((x) => /theme\.localhost|null/.test(x.origin) && x.auxData && !x.auxData.isDefault) || ctxs.find((x) => x.origin !== "http://tauri.localhost");
    const r = await c.send("Runtime.evaluate", { expression: js, awaitPromise: true, returnByValue: true, contextId: ctx && ctx.id });
    c.close();
    return r;
  }
  const c = await connect(target.webSocketDebuggerUrl);
  const r = await c.send("Runtime.evaluate", { expression: js, awaitPromise: true, returnByValue: true });
  c.close();
  return r;
}

(async () => {
  if (cmd === "targets") {
    console.log(JSON.stringify((await targets()).map((t) => ({ type: t.type, url: t.url })), null, 1));
  } else if (cmd === "shot") {
    const { target } = await page();
    const c = await connect(target.webSocketDebuggerUrl);
    const r = await c.send("Page.captureScreenshot", { format: "png" });
    const fs = await import("node:fs");
    fs.writeFileSync(a1 || "shot.png", Buffer.from(r.data, "base64"));
    console.log("saved", a1 || "shot.png");
    c.close();
  } else if (cmd === "eval") {
    const r = await evalIn(a1, a2 === "frame");
    console.log(JSON.stringify(r.result.value ?? r.result, null, 1));
    if (r.exceptionDetails) console.log("EXCEPTION", JSON.stringify(r.exceptionDetails.exception?.description || r.exceptionDetails));
  } else if (cmd === "console") {
    const { all } = await page();
    const secs = Number(a1 || 5);
    for (const t of all.filter((t) => t.type === "page" || t.type === "iframe")) {
      const c = await connect(t.webSocketDebuggerUrl);
      c.on((m) => {
        if (m.method === "Runtime.consoleAPICalled") console.log(`[${t.type}] ${m.params.type}:`, m.params.args.map((x) => x.value ?? x.description).join(" "));
        if (m.method === "Runtime.exceptionThrown") console.log(`[${t.type}] EXCEPTION:`, m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
        if (m.method === "Log.entryAdded") console.log(`[${t.type}] log ${m.params.entry.level}:`, m.params.entry.text, m.params.entry.url || "");
      });
      await c.send("Runtime.enable");
      await c.send("Log.enable");
    }
    await new Promise((r) => setTimeout(r, secs * 1000));
    process.exit(0);
  } else {
    console.log("usage: targets | shot out.png | eval <js> [frame] | console <secs>");
  }
})().catch((e) => {
  console.error("cdp error:", e.message);
  process.exit(1);
});
