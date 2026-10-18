// Starts the real Arcalo Mail desktop binary under tauri-driver and returns a WebdriverIO
// session, as Arcalo's e2e harness does. Each launch uses a fresh, isolated data folder. The
// mail servers are the test server on this computer (lib/server.js); nothing goes to the
// internet.
//
// Processes: only what this file started is stopped (tauri-driver runs in its own process
// group with the WebDriver and the app below it; Xvfb only when this run started it). Nothing
// is killed by name.

import { spawn, execSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { remote } from "webdriverio";

const ROOT = path.resolve(import.meta.dirname, "../..");
export const APP = path.resolve(process.env.ARCALO_MAIL_APP ?? path.join(ROOT, "target/debug/arcalo-mail"));
export const SHOTS = process.env.ARCALO_MAIL_SHOTS ?? path.join(ROOT, "e2e/screenshots");
const DISPLAY = process.env.DISPLAY || ":76";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let xvfb = null;
function ensureXvfb() {
  try {
    execSync(`xdpyinfo -display ${DISPLAY}`, { stdio: "ignore" });
  } catch {
    xvfb = spawn("Xvfb", [DISPLAY, "-screen", "0", "2200x1400x24", "-nolisten", "tcp"], { stdio: "ignore" });
    xvfb.unref();
    process.on("exit", () => xvfb?.kill());
    execSync("sleep 1");
  }
}

/** A session bus of this run's own: no keyring, portal or notification service (the app keeps
 *  passwords in its protected file then, as on a desktop without a keyring). */
let busAddress = null;
function sessionBus() {
  if (busAddress !== null) return busAddress;
  busAddress = "";
  const conf = path.join(os.tmpdir(), `arcalo-mail-e2e-bus-${process.pid}.conf`);
  try {
    fs.writeFileSync(
      conf,
      `<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:tmpdir=${os.tmpdir()}</listen>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>`,
    );
    const out = execFileSync("dbus-daemon", ["--config-file", conf, "--fork", "--print-address=1", "--print-pid=1"], { encoding: "utf8", timeout: 5000 });
    const [address, pid] = out.trim().split("\n");
    process.on("exit", () => {
      try {
        process.kill(Number(pid));
      } catch {
        /* gone */
      }
    });
    busAddress = address;
  } catch {
    /* no dbus-daemon */
  } finally {
    fs.rmSync(conf, { force: true });
  }
  return busAddress;
}

/** Programs the app would open (a link, an attachment) are not started: `xdg-open` only notes
 *  its arguments in $ARCALO_MAIL_E2E_OPENED. */
let shimDir = null;
function openerShim() {
  if (shimDir) return shimDir;
  shimDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-open-"));
  fs.writeFileSync(path.join(shimDir, "xdg-open"), '#!/bin/sh\nprintf \'%s\\n\' "$*" >> "${ARCALO_MAIL_E2E_OPENED:-/dev/null}"\n', { mode: 0o755 });
  process.on("exit", () => fs.rmSync(shimDir, { recursive: true, force: true }));
  return shimDir;
}

const portFree = (port) =>
  new Promise((resolve) => {
    const srv = net.createServer();
    srv.once("error", () => resolve(false));
    srv.listen(port, "127.0.0.1", () => srv.close(() => resolve(true)));
  });

async function driverPort() {
  for (let i = 0; i < 50; i++) {
    const port = 5444 + Math.floor(Math.random() * 500);
    if ((await portFree(port)) && (await portFree(port + 1000))) return port;
  }
  throw new Error("no free port for tauri-driver");
}

async function waitForPort(port, timeout = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/status`);
      if (r.ok) return;
    } catch {
      /* not up yet */
    }
    await sleep(150);
  }
  throw new Error(`port ${port} did not open`);
}

/** Wraps node:test's `test` so a failing test leaves a screenshot behind. */
export function guarded(test, getApp) {
  return (name, fn) =>
    test(name, async (t) => {
      try {
        await fn(t);
      } catch (e) {
        const app = getApp();
        const slug = name.replace(/[^a-z0-9]+/gi, "-").slice(0, 60);
        await app?.browser.saveScreenshot(path.join(SHOTS, `FAIL-${slug}.png`)).catch(() => {});
        throw e;
      }
    });
}

let launches = 0;

/**
 * Starts the app. `locale`: the system language it sees; `env`: extra variables (a fixed clock
 * for screenshots: ARCALO_MAIL_NOW is read by the UI's date formatting only in tests).
 */
export async function launch({ width = 1440, height = 900, locale = "de-DE", env: extra = {}, dataDir: chosen = null } = {}) {
  ensureXvfb();
  fs.mkdirSync(SHOTS, { recursive: true });
  const dataDir = chosen ?? fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-"));
  const port = await driverPort();
  const opened = path.join(os.tmpdir(), `arcalo-mail-e2e-opened-${process.pid}-${++launches}.log`);
  const bus = sessionBus();
  const shim = openerShim();
  const env = {
    ...process.env,
    DISPLAY,
    ...(bus ? { DBUS_SESSION_BUS_ADDRESS: bus } : {}),
    PATH: `${shim}${path.delimiter}${process.env.PATH ?? ""}`,
    BROWSER: path.join(shim, "xdg-open"),
    ARCALO_MAIL_DATA_DIR: dataDir,
    ARCALO_MAIL_SECRET_STORE: "file",
    ARCALO_MAIL_LOCALE: locale,
    XDG_DATA_HOME: path.join(dataDir, "xdg-data"),
    XDG_CACHE_HOME: path.join(dataDir, "xdg-cache"),
    XDG_CONFIG_HOME: path.join(dataDir, "xdg-config"),
    WEBKIT_DISABLE_COMPOSITING_MODE: "1",
    GDK_BACKEND: "x11",
    NO_AT_BRIDGE: "1",
    ARCALO_MAIL_E2E_OPENED: opened,
    ...extra,
  };
  const driver = spawn("tauri-driver", ["--port", String(port), "--native-port", String(port + 1000)], { env, stdio: ["ignore", "ignore", "pipe"], detached: true });
  let driverErr = "";
  driver.stderr.on("data", (d) => (driverErr += d));
  const stopDriver = () => {
    try {
      // The driver's process group: tauri-driver, WebKitWebDriver and the app this run started.
      process.kill(-driver.pid, "SIGTERM");
    } catch {
      /* gone */
    }
  };
  process.on("exit", stopDriver);
  await waitForPort(port);
  const browser = await remote({
    hostname: "127.0.0.1",
    port,
    logLevel: "error",
    capabilities: { "wdio:enforceWebDriverClassic": true, "tauri:options": { application: APP } },
  });
  await browser.setWindowSize(width, height).catch(() => {});
  await browser.waitUntil(async () => (await browser.execute(() => document.body.classList.contains("ready"))) === true, {
    timeout: 20000,
    timeoutMsg: "app did not become ready",
  });

  const app = {
    browser,
    dataDir,
    /** The environment the app runs in (a second start of the program needs the same). */
    env,
    async shot(name, dir = SHOTS) {
      await sleep(300);
      fs.mkdirSync(dir, { recursive: true });
      await browser.saveScreenshot(path.join(dir, `${name}.png`));
    },
    $: (sel) => browser.$(sel),
    $$: (sel) => browser.$$(sel),
    async click(sel) {
      const el = await browser.$(sel);
      await el.waitForClickable({ timeout: 8000 });
      // A sheet still sliding in moves its buttons between the press and the release: wait
      // until the element stands still (two frames at the same place), as a finger would.
      await browser.executeAsync((e, done) => {
        const until = performance.now() + 1500;
        let last = "";
        const f = () => {
          const r = e.getBoundingClientRect();
          const now = `${r.left},${r.top},${r.width},${r.height}`;
          if (now === last || performance.now() > until) return done();
          last = now;
          requestAnimationFrame(() => requestAnimationFrame(f));
        };
        f();
      }, el);
      await el.click();
      return el;
    },
    async textOf(el) {
      return (await browser.execute((e) => e.innerText, el)).trim();
    },
    async text(sel) {
      return app.textOf(await browser.$(sel));
    },
    async keys(k) {
      await browser.keys(k);
    },
    async type(text) {
      for (const ch of text) await browser.keys(ch);
    },
    /** Sets a field's value as typing would (React sees the input event). */
    async fill(sel, value) {
      const el = await app.waitFor(sel);
      await el.click();
      await browser.execute(
        (e, v) => {
          const proto = e.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
          Object.getOwnPropertyDescriptor(proto, "value").set.call(e, v);
          e.dispatchEvent(new Event("input", { bubbles: true }));
        },
        el,
        value,
      );
    },
    async invoke(cmd, args = {}) {
      const r = await browser.executeAsync(
        (c, a, done) => window.__TAURI_INTERNALS__.invoke(c, a).then((x) => done({ ok: x }), (e) => done({ err: String(e) })),
        cmd,
        args,
      );
      if (r.err) throw new Error(r.err);
      return r.ok;
    },
    async waitFor(sel, timeout = 10000) {
      await browser.waitUntil(
        async () => {
          try {
            return await (await browser.$(sel)).isDisplayed();
          } catch (e) {
            if (/stale element/i.test(String(e?.message ?? e))) return false;
            throw e;
          }
        },
        { timeout, timeoutMsg: `${sel} not displayed after ${timeout}ms` },
      );
      return browser.$(sel);
    },
    async waitText(sel, pattern, timeout = 10000) {
      await browser.waitUntil(
        async () => {
          for (const e of await browser.$$(sel)) if (pattern.test(await app.textOf(e))) return true;
          return false;
        },
        { timeout, timeoutMsg: `no ${sel} matching ${pattern}` },
      );
    },
    async waitUntil(f, timeout = 10000, msg = "condition") {
      await browser.waitUntil(f, { timeout, interval: 150, timeoutMsg: msg });
    },
    opened() {
      return fs.existsSync(opened) ? fs.readFileSync(opened, "utf8").split("\n").filter(Boolean) : [];
    },
    async close() {
      await browser.deleteSession().catch(() => {});
      stopDriver();
      await sleep(300);
      if (!chosen) fs.rmSync(dataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
      fs.rmSync(opened, { force: true });
      if (driverErr.includes("panicked")) throw new Error(driverErr);
    },
  };
  return app;
}
