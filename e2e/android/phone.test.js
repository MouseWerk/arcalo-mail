// The Android app on an emulator, measured: the debug APK (ARCALO_MAIL_APK) at 360×800 and
// 412×915 dp, portrait, against the Gmail-like test server on the host (adb reverse). Every main
// screen (setup, inbox, folders, message, compose, settings, calendar) is as wide as the screen,
// nothing wider, nothing tappable under the status or navigation bar (their frames from
// `dumpsys window`, the WebView's from the view tree); spinners stay small and round; the compose
// button leaves the last row free at the end of the list; every inbox tab can be reached;
// writing has one bar on top with the send button and, with the keyboard up (a real tap into
// the text), one row of tools above it; folder names keep their brackets. The checks are the
// desktop phone test's (e2e/lib/geometry.js). No stock Android UI shows on any screen
// (lib/stockui.js: platform form controls and pickers, alert/confirm/prompt, tap highlight,
// selectable chrome, focus rings after a tap, overscroll stretch, scrollbars, search and e-mail
// fields' keyboards), and with the system's font size at 130 % no label is cut off. Screenshots of the whole screen land in
// e2e/screenshots/android.
//
//   cd e2e && ARCALO_MAIL_APK=path/to/app-debug.apk node --test android/phone.test.js

import { describe, test, after, before } from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { startServer } from "../lib/server.js";
import * as geo from "../lib/geometry.js";
import * as stock from "../lib/stockui.js";
import * as device from "./device.js";

const OUT = path.resolve(import.meta.dirname, "../screenshots/android");
const APK = process.env.ARCALO_MAIL_APK;
const TABS = ["people", "newsletters", "notifications", "receipts"];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let server;
let fixture;

before(async () => {
  server = await startServer();
  fixture = await server.gmail(60);
  await server.slow({ headers: 300, oldBodies: 1500 });
  if (APK) device.install(APK);
  device.reverse([server.imap, server.smtp, server.control]);
});

after(() => {
  server?.stop();
  try {
    device.resetScreen();
  } catch {
    /* the emulator is gone */
  }
});

for (const [W, H] of [
  [360, 800],
  [412, 915],
]) {
  describe(`Android phone ${W}×${H}`, () => {
    let page;
    let dpr = 2;
    const spinnerLog = [];
    /** The platform's own UI found on the screens so far. */
    const stockFound = [];

    const shot = (name) => device.screencap(path.join(OUT, `android-${W}x${H}-${name}.png`));
    const visible = (sel, timeout = 30000) => page.waitForSelector(sel, { timeout });
    const keepSpinners = async () => spinnerLog.push(...(await page.evaluate(geo.recordedSpinners).catch(() => [])));

    /** The layout checks of the screen shown now, with the bars where they are now. */
    const screen = async (name, opts) => {
      await sleep(700);
      shot(name);
      const where = device.insetsCss(dpr);
      const m = await page.evaluate(geo.measureLayout, where.insets);
      const out = geo.layoutProblems(m, opts);
      stockFound.push(...(await page.evaluate(stock.measureStockUi)).map((f) => `${name}: ${f}`));
      if (Math.abs(m.viewport.w - W) > 1) out.push(`the page is ${m.viewport.w} px wide on a ${W} dp screen`);
      return out.map((p) => `${name}: ${p}`);
    };

    const reload = async () => {
      await keepSpinners();
      await page.reload();
      await page.waitForFunction(() => document.body.classList.contains("ready"), undefined, { timeout: 60000 });
    };

    /** Taps an element with a real finger (adb), where it is on the screen. */
    const tapOn = async (sel) => {
      const r = await page.evaluate((s) => document.querySelector(s)?.getBoundingClientRect().toJSON() ?? null, sel);
      assert.ok(r, `no ${sel}`);
      const wv = device.webViewBounds();
      device.tap(wv.left + (r.left + Math.min(r.width / 2, 40)) * dpr, wv.top + (r.top + Math.min(r.height / 2, 30)) * dpr);
    };

    before(async () => {
      try {
        await open();
      } catch (e) {
        // What the device says, for the artifact and the log.
        console.error(`could not open the app at ${W}×${H}: ${e?.stack ?? e}`);
        device.diagnose(path.join(OUT, `android-${W}x${H}-diagnose.txt`));
        throw e;
      }
    });

    const open = async () => {
      const step = (what) => console.log(`${W}×${H}: ${what}`);
      device.fresh();
      device.setScreen(W, H, 2);
      const pid = await device.start();
      step(`started, pid ${pid}`);
      page = await device.connect(pid);
      await page.addInitScript(geo.recordSpinners);
      await page.addInitScript(stock.stubDialogs);
      // The app in German whatever the emulator's language (the labels the steps look for).
      await page.waitForFunction(() => window.__TAURI_INTERNALS__ != null, undefined, { timeout: 60000 });
      step("the page has the app's bridge");
      await page.evaluate(async () => {
        const s = await window.__TAURI_INTERNALS__.invoke("get_settings", {});
        await window.__TAURI_INTERNALS__.invoke("set_settings", { settings: { ...s, language: "de" } });
      });
      step("language set");
      await page.reload();
      await page.waitForFunction(() => document.body.classList.contains("ready"), undefined, { timeout: 60000 });
      step("ready after the reload");
      await page.evaluate(geo.recordSpinners);
      dpr = await page.evaluate(() => window.devicePixelRatio);
      console.log(`connected at ${W}×${H}: ${page.url()}, ${await page.evaluate(() => `${innerWidth}×${innerHeight}`)} at ${dpr}x`);
    };

    after(async () => {
      await page?.close();
      device.unforward();
    });

    test("fills the screen at the first start", async () => {
      await visible(".setup-welcome", 60000);
      const problems = await screen("setup");
      assert.deepEqual(problems, []);
    });

    test("shows the inbox with every tab in reach and the last row clear of the compose button", async () => {
      await page.evaluate(
        ([user, password, transport]) =>
          window.__TAURI_INTERNALS__.invoke("add_account", {
            draft: { provider: "imap", name: "", email: user, sender_name: "Maria Muster", transport },
            password,
          }),
        [server.user, server.password, server.transport()],
      );
      await reload();
      await page.waitForFunction(() => document.querySelectorAll(".row:not(.row-placeholder)").length >= 5, undefined, { timeout: 90000 });
      await page.waitForFunction((n) => document.querySelectorAll(".inbox-tab").length >= n, TABS.length, { timeout: 60000 });
      const problems = await screen("inbox");
      problems.push(...geo.tabProblems(await page.evaluate(geo.measureTabs), TABS));
      for (let i = 0; i < 20; i++) {
        const r = await page.evaluate(geo.scrollListToEnd);
        await sleep(400);
        if (!r.moved) break;
      }
      await sleep(500);
      shot("inbox-end");
      problems.push(...geo.fabProblems(await page.evaluate(geo.measureFab)));
      assert.deepEqual(problems, []);
    });

    test("lists the folders by their whole names", async () => {
      await page.evaluate(geo.scrollListToStart);
      await page.click(".folders-btn");
      await visible(".sidebar");
      await page.waitForFunction(() => document.querySelectorAll(".account .folder-name").length >= 8, undefined, { timeout: 30000 });
      const problems = await screen("folders");
      problems.push(...geo.nameProblems(await page.evaluate(geo.measureNames, ".account .folder-name"), ["[Notion]", "Inbox", "Belege"]));
      await page.click(".folder-name", { text: /^Posteingang$/ });
      await visible(".list-pane");
      assert.deepEqual(problems, []);
    });

    test("shows a message on a screen that fits", async () => {
      await page.click(".row", { text: new RegExp(fixture.cid) });
      await visible(".message-subject");
      await page.waitForFunction(() => document.querySelectorAll(".message-loading").length === 0, undefined, { timeout: 30000 });
      const problems = await screen("message");
      device.back();
      await visible(".list-pane");
      assert.deepEqual(problems, []);
    });

    test("writes with one bar on top and one row of tools above the keyboard", async () => {
      await page.click(".compose-fab");
      await visible(".compose");
      const problems = await screen("compose");
      problems.push(...geo.composeProblems(await page.evaluate(geo.measureCompose)));
      const before = await page.evaluate(() => window.innerHeight);
      await tapOn(".compose .editor");
      const t0 = Date.now();
      while (Date.now() - t0 < 15000 && !(device.keyboardShown() && (await page.evaluate(() => window.innerHeight)) < before - 100)) await sleep(300);
      assert.ok(device.keyboardShown(), "the keyboard did not come up");
      problems.push(...(await screen("compose-keyboard")));
      problems.push(...geo.composeProblems(await page.evaluate(geo.measureCompose), { keyboard: true }).map((p) => `with the keyboard: ${p}`));
      device.back();
      await sleep(500);
      if (device.keyboardShown()) device.back();
      device.back();
      await page.waitForFunction(() => !document.querySelector(".compose"), undefined, { timeout: 15000 }).catch(() => {});
      if (await page.$(".compose")) {
        // A draft was kept: discard it.
        await page.click('.compose-foot [aria-label="Verwerfen"]', { timeout: 3000 }).catch(() => {});
      }
      assert.deepEqual(problems, []);
    });

    test("opens the settings and the calendar on screens that fit", async () => {
      await page.click(".folders-btn");
      await page.click('.sidebar-head [aria-label="Einstellungen"]');
      await visible(".settings-nav");
      const problems = await screen("settings");
      await page.click(".settings-nav-item", { text: /^Lesen$/ });
      await visible(".settings-main");
      problems.push(...(await screen("settings-reading")));
      device.back();
      await sleep(400);
      device.back();
      await page.waitForFunction(() => !document.querySelector(".settings"), undefined, { timeout: 10000 });
      if (!(await page.$(".views .folder"))) await page.click(".folders-btn");
      await page.click(".views .folder", { text: /^Kalender/ });
      await visible(".calv-head");
      problems.push(...(await screen("calendar")));
      assert.deepEqual(problems, []);
    });

    test("shows no stock Android UI", async () => {
      const dialogs = await page.evaluate(stock.nativeDialogs);
      assert.deepEqual([...new Set(stockFound)], [], "the platform's own UI");
      assert.deepEqual(dialogs, [], "alert, confirm or prompt called");
    });

    test("keeps every label whole with the system's font at 130 %", async () => {
      device.shell("settings put system font_scale 1.3");
      try {
        await sleep(1500);
        const cut = [];
        const look = async (name) => {
          await sleep(800);
          shot(`font-130-${name}`);
          cut.push(...(await page.evaluate(stock.measureStockUi)).filter((f) => f.startsWith("text cut off")).map((f) => `${name}: ${f}`));
        };
        if (!(await page.$(".list-pane .row"))) await page.click(".folders-btn").catch(() => {});
        if (await page.$(".folder-name")) await page.click(".folder-name", { text: /^Posteingang$/, timeout: 5000 }).catch(() => {});
        await look("inbox");
        await page.click(".folders-btn");
        await look("folders");
        await page.click('.sidebar-head [aria-label="Einstellungen"]');
        await visible(".settings-nav");
        await look("settings");
        device.back();
        await sleep(500);
        await page.click(".compose-fab").catch(() => {});
        if (await page.$(".compose")) await look("compose");
        assert.deepEqual(cut, []);
      } finally {
        device.shell("settings put system font_scale 1.0");
      }
    });

    test("draws every spinner small and round", async () => {
      await keepSpinners();
      assert.deepEqual(geo.spinnerProblems(spinnerLog), []);
    });
  });
}
