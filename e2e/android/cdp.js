// A small client of the Chrome DevTools Protocol for the app's page in the Android WebView. A
// browser-level client (Playwright's connectOverCDP) asks the WebView for browser contexts it
// does not have; a page target needs only Runtime, Page and Input, which every WebView offers.
// The calls mirror the part of Playwright's page the phone tests use.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The call of a page-side function with its argument, as an expression. */
const call = (fn, arg) => `(${fn.toString()})(${arg === undefined ? "" : JSON.stringify(arg)})`;

export class Page {
  constructor(ws, url) {
    this.ws = ws;
    this.lastUrl = url;
    this.next = 1;
    this.waiting = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(typeof ev.data === "string" ? ev.data : Buffer.from(ev.data).toString());
      const w = msg.id && this.waiting.get(msg.id);
      if (!w) return;
      this.waiting.delete(msg.id);
      if (msg.error) w.reject(new Error(`${w.method}: ${msg.error.message}`));
      else w.resolve(msg.result);
    });
  }

  /** Connects to the page target whose address matches `match` on the forwarded port. */
  static async connect(port, match) {
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const t = targets.find((x) => x.type === "page" && match.test(x.url));
    if (!t) throw new Error(`no page of the app among ${JSON.stringify(targets.map((x) => `${x.type} ${x.url}`))}`);
    const url = `ws://127.0.0.1:${port}${new URL(t.webSocketDebuggerUrl).pathname}`;
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`could not open ${url}`)), { once: true });
    });
    const page = new Page(ws, t.url);
    await page.send("Page.enable");
    await page.send("Runtime.enable");
    return page;
  }

  /** One call; it fails after `timeout` ms without an answer (a page that went away). */
  send(method, params = {}, timeout = 60000) {
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting.delete(id);
        reject(new Error(`${method}: no answer after ${timeout} ms`));
      }, timeout);
      const done = (f) => (v) => {
        clearTimeout(timer);
        f(v);
      };
      this.waiting.set(id, { resolve: done(resolve), reject: done(reject), method });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  url() {
    return this.lastUrl;
  }

  async evaluate(fn, arg) {
    const r = await this.send("Runtime.evaluate", { expression: call(fn, arg), awaitPromise: true, returnByValue: true }, 30000);
    if (r.exceptionDetails) throw new Error(`in the page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }

  async waitForFunction(fn, arg = undefined, { timeout = 30000 } = {}) {
    const t0 = Date.now();
    let last = null;
    while (Date.now() - t0 < timeout) {
      try {
        const v = await this.evaluate(fn, arg);
        if (v) return v;
      } catch (e) {
        // A page that is loading again.
        last = e;
      }
      await sleep(200);
    }
    throw new Error(`timed out after ${timeout} ms waiting for ${fn.toString().slice(0, 120)}${last ? ` (${last.message})` : ""}`);
  }

  waitForSelector(sel, { timeout = 30000 } = {}) {
    return this.waitForFunction(
      (s) => {
        const e = document.querySelector(s);
        if (!e) return false;
        const r = e.getBoundingClientRect();
        const cs = getComputedStyle(e);
        return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none";
      },
      sel,
      { timeout },
    );
  }

  async $(sel) {
    return (await this.evaluate((s) => document.querySelector(s) != null, sel)) ? sel : null;
  }

  async reload() {
    await this.send("Page.reload", {});
    await sleep(500);
    await this.waitForFunction(() => document.readyState === "complete", undefined, { timeout: 60000 });
  }

  async addInitScript(fn) {
    await this.send("Page.addScriptToEvaluateOnNewDocument", { source: call(fn) });
  }

  /** A mouse click (pressed and released) in the middle of the first element `sel` matches
   *  whose text, when given, matches `text`. */
  async click(sel, { text = null, timeout = 30000 } = {}) {
    const at = await this.clickTarget(sel, text, timeout).catch((e) => {
      throw new Error(`no ${sel}${text ? ` with ${text}` : ""} to click: ${e.message.split("\n")[0].slice(0, 120)}`);
    });
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
      await this.send("Input.dispatchMouseEvent", { type, x: at.x, y: at.y, button: "left", clickCount: type === "mouseMoved" ? 0 : 1 });
    }
  }

  clickTarget(sel, text, timeout) {
    return this.waitForFunction(
      ([s, src, flags]) => {
        const re = src == null ? null : new RegExp(src, flags);
        for (const e of document.querySelectorAll(s)) {
          if (re && !re.test(e.textContent.trim())) continue;
          e.scrollIntoView({ block: "nearest" });
          const r = e.getBoundingClientRect();
          if (r.width > 0 && r.height > 0) return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
        }
        return null;
      },
      [sel, text?.source ?? null, text?.flags ?? ""],
      { timeout },
    );
  }

  /** A key pressed and released (`Escape`, `Enter`). */
  async key(key) {
    const codes = { Escape: 27, Enter: 13, Tab: 9 };
    for (const type of ["keyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: codes[key] ?? 0 });
    }
  }

  /** A mouse click at a point of the page. */
  async clickAt(x, y) {
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
      await this.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: type === "mouseMoved" ? 0 : 1 });
    }
  }

  async close() {
    try {
      this.ws.close();
    } catch {
      /* closed */
    }
  }
}
