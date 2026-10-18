// A screenshot tour of the Android app on the emulator, for review and as the baseline of each
// screen: every screen, every menu that opens from it (buttons with aria-haspopup), the sheets
// and dialogs, at 360×800 and 412×915 dp, light and dark. The whole screen is captured (status
// and navigation bar included) to e2e/screenshots/android/tour-<size>-<theme>/NN-<name>.png.
// On every picture the page is also checked for the platform's own UI (lib/stockui.js); the
// findings are listed in e2e/screenshots/android/stock-ui-findings.txt and printed. The tour
// fails only when it cannot go on; the checks that fail are android/phone.test.js's.
//
//   cd e2e && ARCALO_MAIL_APK=path/to/app-debug.apk node --test android/tour.test.js

import { test as nodeTest, after, before } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { guarded } from "../lib/harness.js";
import { startServer } from "../lib/server.js";
import * as stock from "../lib/stockui.js";
import * as device from "./device.js";

const OUT = path.resolve(import.meta.dirname, "../screenshots/android");
const APK = process.env.ARCALO_MAIL_APK;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// A failing test leaves the phone's screen behind (FAIL-*.png), as the desktop tests do.
const test = guarded(nodeTest, () => ({ browser: { saveScreenshot: async (file) => device.screencap(file) } }));
const findings = new Map();

let server;
let fixture;

before(async () => {
  server = await startServer();
  fixture = await server.gmail(60);
  if (APK) device.install(APK);
  device.reverse([server.imap, server.smtp, server.control]);
});

after(() => {
  server?.stop();
  const lines = [...findings.entries()].map(([f, where]) => `${f}\n    on: ${[...where].join(", ")}`);
  fs.mkdirSync(OUT, { recursive: true });
  fs.writeFileSync(path.join(OUT, "stock-ui-findings.txt"), `${lines.join("\n")}\n`);
  console.log(`stock UI findings (${lines.length}):\n${lines.join("\n")}`);
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
  for (const theme of ["light", "dark"]) {
    test(`tour at ${W}×${H}, ${theme}`, async () => {
      const dir = path.join(OUT, `tour-${W}x${H}-${theme}`);
      let n = 0;
      device.fresh();
      device.setScreen(W, H, 2);
      let page = await device.connect(await device.start());
      try {
        await sleep(3000);
        await page.addInitScript(stock.stubDialogs);
        await page.waitForFunction(() => window.__TAURI_INTERNALS__ != null, undefined, { timeout: 60000 });
        await page.evaluate(async (t) => {
          const s = await window.__TAURI_INTERNALS__.invoke("get_settings", {});
          await window.__TAURI_INTERNALS__.invoke("set_settings", { settings: { ...s, language: "de", theme: t } });
        }, theme);
        const ready = async () => {
          await page.reload();
          await page.waitForFunction(() => document.body.classList.contains("ready"), undefined, { timeout: 60000 });
        };
        await ready();

        /** The whole screen as it is now, and what the page shows of the platform's UI. */
        const shot = async (name) => {
          device.dismissSystemDialogs();
          await sleep(700);
          const file = `${String(++n).padStart(2, "0")}-${name}`;
          device.screencap(path.join(dir, `${file}.png`));
          const found = [
            ...(await page.evaluate(stock.measureStockUi).catch((e) => [`(not measured: ${e.message})`])),
            ...(await page.evaluate(stock.nativeDialogs).catch(() => [])),
          ];
          for (const f of found) {
            if (!findings.has(f)) findings.set(f, new Set());
            findings.get(f).add(`${W}x${H}-${theme}/${file}`);
          }
        };
        /** Every menu the screen offers: opened, pictured, closed again. */
        const menus = async (where) => {
          const count = await page.evaluate(() => [...document.querySelectorAll("[aria-haspopup]")].filter((b) => b.getBoundingClientRect().width > 0).length);
          for (let i = 0; i < count; i++) {
            const at = await page.evaluate((k) => {
              const b = [...document.querySelectorAll("[aria-haspopup]")].filter((x) => x.getBoundingClientRect().width > 0)[k];
              if (!b) return null;
              const r = b.getBoundingClientRect();
              return { x: r.left + r.width / 2, y: r.top + r.height / 2, label: (b.getAttribute("aria-label") || b.textContent || "").trim().slice(0, 24) };
            }, i);
            if (!at) continue;
            await page.clickAt(at.x, at.y);
            await shot(`${where}-menu-${at.label.replace(/[^a-z0-9äöüß]+/gi, "-").toLowerCase() || i}`);
            await page.key("Escape");
            await sleep(300);
          }
        };
        const back = async () => {
          device.back();
          await sleep(600);
        };
        /** One part of the tour: a failure is noted (with a picture) and the tour goes on from
         *  the inbox. */
        const broken = [];
        const section = async (name, f) => {
          try {
            await f();
          } catch (e) {
            broken.push(`${name}: ${e.message.split("\n")[0]}`);
            device.screencap(path.join(dir, `FAIL-${name}.png`));
            await page.key("Escape").catch(() => {});
            for (let i = 0; i < 3 && !(await page.$(".list-pane .row").catch(() => null)); i++) await back();
          }
        };

        await page.waitForSelector(".setup-welcome", { timeout: 60000 });
        await shot("setup");
        await page.evaluate(
          ([user, password, transport]) =>
            window.__TAURI_INTERNALS__.invoke("add_account", {
              draft: { provider: "imap", name: "", email: user, sender_name: "Maria Muster", transport },
              password,
            }),
          [server.user, server.password, server.transport()],
        );
        await ready();
        await page.addInitScript(stock.stubDialogs);
        await page.waitForFunction(() => document.querySelectorAll(".row:not(.row-placeholder)").length >= 5, undefined, { timeout: 90000 });
        await shot("inbox");
        await menus("inbox");
        for (const tab of await page.evaluate(() => [...document.querySelectorAll(".inbox-tab")].map((t) => t.dataset.tab))) {
          await page.click(`.inbox-tab[data-tab="${tab}"]`);
          await shot(`inbox-${tab}`);
        }
        await page.click('.inbox-tab[data-tab="people"]').catch(() => {});

        // Search.
        await section("search", async () => {
          if (await page.$(".search-input")) {
            await page.click(".search-input");
            await sleep(800);
            await shot("search-keyboard");
            await back();
            await page.key("Escape");
            await page.evaluate(() => document.querySelector(".search-clear")?.click());
          }
        });

        // The message and its menus.
        await section("message", async () => {
          await page.click('.inbox-tab[data-tab="people"]').catch(() => {});
          await page.evaluate(() => {
            const s = document.querySelector(".messages");
            if (s) s.scrollTop = 0;
          });
          await page.click(".row", { text: new RegExp(fixture.cid) });
          await page.waitForSelector(".message-subject");
          await shot("message");
          await menus("message");
          await back();
        });

        // The folders and a new folder's dialog.
        await section("folders", async () => {
          await page.click(".folders-btn");
          await page.waitForSelector(".sidebar");
          await shot("folders");
          await menus("folders");
          if (await page.$(".account-more")) {
            await page.click(".account-more");
            await shot("new-folder-dialog");
            await page.key("Escape");
            await back();
          }
        });

        // Writing.
        await section("compose", async () => {
          if (!(await page.$(".compose-fab"))) await page.click(".folder-name", { text: /^Posteingang$/ });
          await page.click(".compose-fab");
          await page.waitForSelector(".compose");
          await shot("compose");
          await menus("compose");
          const r = await page.evaluate(() => document.querySelector(".compose .editor")?.getBoundingClientRect().toJSON() ?? null);
          const wv = device.webViewBounds();
          const dpr = await page.evaluate(() => window.devicePixelRatio);
          if (r) device.tap(wv.left + (r.left + 40) * dpr, wv.top + (r.top + 20) * dpr);
          await sleep(1500);
          await shot("compose-keyboard");
          await back();
          await back();
          if (await page.$(".compose")) {
            await shot("compose-close-dialog");
            await page.click('[aria-label="Verwerfen"]', { timeout: 3000 }).catch(() => {});
            await sleep(500);
          }
        });

        // The settings, each section.
        await section("settings", async () => {
          await page.click(".folders-btn");
          await page.click('.sidebar-head [aria-label="Einstellungen"]');
          await page.waitForSelector(".settings-nav");
          await shot("settings");
          const sections = await page.evaluate(() => [...document.querySelectorAll(".settings-nav-item")].map((b) => b.textContent.trim()));
          for (const section of sections) {
            await page.click(".settings-nav-item", { text: new RegExp(`^${section.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) });
            await page.waitForSelector(".settings-main").catch(() => {});
            await shot(`settings-${section.toLowerCase().replace(/[^a-z0-9äöüß]+/g, "-")}`);
            await menus(`settings-${section.toLowerCase().replace(/[^a-z0-9äöüß]+/g, "-")}`);
            await back();
          }
          await back();
        });

        // The app lock: a PIN set in „Sicherheit“, the lock screen, a wrong PIN, the right one.
        const fill = (sel, value) =>
          page.evaluate(
            ([s, v]) => {
              const e = document.querySelector(s);
              e.focus();
              Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(e, v);
              e.dispatchEvent(new Event("input", { bubbles: true }));
            },
            [sel, value],
          );
        await section("lock", async () => {
          await page.click(".folders-btn").catch(() => {});
          await page.click('.sidebar-head [aria-label="Einstellungen"]');
          await page.click(".settings-nav-item", { text: /^Sicherheit$/ });
          await page.click("[data-security] [role=radio]", { text: /^Bei Inaktivität$/ });
          await page.waitForSelector("[data-pin-new]");
          await fill("[data-pin-new]", "4711");
          await fill("[data-pin-repeat]", "4711");
          await shot("security-pin");
          await page.click("[data-pin-save]");
          await shot("security-pin-saved");
          await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("applock_lock_now", {}));
          await page.waitForSelector(".lock-screen");
          await shot("lock-screen");
          const pin = await page.evaluate(() => document.querySelector(".lock-pin")?.getBoundingClientRect().toJSON() ?? null);
          if (pin) {
            const lw = device.webViewBounds();
            device.tap(lw.left + (pin.left + pin.width / 2) * dpr, lw.top + (pin.top + pin.height / 2) * dpr);
            await sleep(1500);
            await shot("lock-pin-keyboard");
            await back();
          }
          await fill(".lock-pin", "0000");
          await page.key("Enter");
          await shot("lock-wrong-pin");
          await fill(".lock-pin", "4711");
          await page.key("Enter");
          await page.waitForFunction(() => !document.querySelector(".lock-screen"), undefined, { timeout: 15000 });
          await page.click("[data-security] [role=radio]", { text: /^Aus$/ }).catch(() => {});
          await back();
          await back();
        });

        // The calendar and its views.
        await section("calendar", async () => {
          if (!(await page.$(".views .folder"))) await page.click(".folders-btn");
          await page.click(".views .folder", { text: /^Kalender/ });
          await page.waitForSelector(".calv-head");
          await shot("calendar");
          const views = await page.evaluate(() => [...document.querySelectorAll(".calv-views button, .calv-head select option")].map((b) => b.textContent.trim()).filter(Boolean));
          for (const v of views.slice(0, 5)) {
            await page.click(".calv-views button", { text: new RegExp(`^${v}$`), timeout: 3000 }).catch(() => {});
            await shot(`calendar-${v.toLowerCase().replace(/[^a-z0-9äöüß]+/g, "-")}`);
          }
          await menus("calendar");
        });
        assert.deepEqual(broken, [], "parts of the tour that could not be shown");
      } finally {
        await page.close();
        device.unforward();
      }
    });
  }
}
