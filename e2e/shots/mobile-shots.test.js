// Screenshots of the phone and tablet layouts (not part of the test suite): the desktop app in
// its touch layout (ARCALO_MAIL_TOUCH=1) at an iPhone's size (390×844), an Android phone's (412×915)
// with its system bars, and an iPad's (820×1180),
// with the sample mailboxes of the README pictures on the fixed clock, light and dark. Every
// picture shows the whole window. Run after building the app and the test server:
//   cd e2e && npm run shots:mobile
// ARCALO_MAIL_SHOTS sets the folder (default docs/screenshots/mobile).

import { test, before, after } from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import * as mail from "../lib/mail.js";
import { NOW, PERSONAL, WORK, fill, personalMail, workMail } from "./mailbox.js";

const OUT = process.env.ARCALO_MAIL_SHOTS ?? path.resolve(import.meta.dirname, "../../docs/screenshots/mobile");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
for (const k of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
  delete process.env[k];
  delete process.env[k.toLowerCase()];
}
const ENV = { ARCALO_MAIL_NOW: String(NOW), TZ: "Europe/London", ARCALO_MAIL_TOUCH: "1" };

let work, personal, app, dataDir;

async function shot(name) {
  await app.browser.execute(() => document.querySelectorAll(".toast .icon-btn").forEach((b) => b.click()));
  await app.browser.execute(() => document.activeElement?.blur());
  await sleep(500);
  await app.shot(name, OUT);
}

async function setTheme(theme) {
  const s = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...s, theme } });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
}

async function inbox() {
  if (await app.browser.execute(() => document.querySelector(".app")?.dataset.view === "message")) await app.browser.back();
  await app.click(".folders-btn");
  await mail.openFolder(app, "Work", "Inbox");
  await app.waitFor(".list-pane");
}

before(async () => {
  work = await startServer({ user: WORK, password: "sample-password" });
  personal = await startServer({ user: PERSONAL, password: "sample-password" });
  await fill(work, workMail());
  await fill(personal, personalMail());
  dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-mobile-shots-"));
  app = await launch({ width: 390, height: 844, locale: "en-GB", env: ENV, dataDir });
  await addAccount(app, work, { name: "Work", sender: "Emma Larsen" });
  await addAccount(app, personal, { name: "Personal", sender: "Emma Larsen" });
  await setTheme("light");
  await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 6, 30000, "first lines in the list");
});

after(async () => {
  await app?.close().catch(() => {});
  work?.stop();
  personal?.stop();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

for (const theme of ["light", "dark"]) {
  test(`phone, ${theme}`, async () => {
    await app.browser.setWindowSize(390, 844);
    await setTheme(theme);
    await inbox();
    await shot(`phone-list-${theme}`);
    await mail.openMessage(app, "Q2 launch plan");
    await shot(`phone-message-${theme}`);
    await app.browser.back();
    await app.click(".folders-btn");
    await app.waitFor(".sidebar");
    await shot(`phone-folders-${theme}`);
    await mail.openFolder(app, "Work", "Inbox");
    // A swipe to the left, held halfway: the row shows what letting go does.
    await sleep(600);
    const row = (await app.$$(".row"))[1];
    await app.browser
      .action("pointer", { id: `swipe-${theme}`, parameters: { pointerType: "mouse" } })
      .move({ origin: row, x: 80, y: 0 })
      .down({ button: 0 })
      .move({ origin: "pointer", x: -60, y: 0, duration: 80 })
      .move({ origin: "pointer", x: -70, y: 0, duration: 80 })
      .perform(true);
    await sleep(300);
    await app.shot(`phone-swipe-${theme}`, OUT);
    // Back to where it started, then let go: the row springs back, nothing is archived.
    await app.browser
      .action("pointer", { id: `swipe-${theme}`, parameters: { pointerType: "mouse" } })
      .move({ origin: "pointer", x: 130, y: 0, duration: 80 })
      .up({ button: 0 })
      .perform();
    await sleep(400);
  });
}

test("phone, writing and the settings", async () => {
  await app.browser.setWindowSize(390, 844);
  await setTheme("light");
  await inbox();
  await mail.openTab(app, "receipts");
  await mail.openMessage(app, "Invoice INV-2026-031");
  await app.click('.touch-actions [aria-label="Reply"]');
  await app.waitFor(".compose");
  await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor");
    ed.focus();
    const r = document.createRange();
    r.setStart(ed.firstChild ?? ed, 0);
    r.collapse(true);
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  });
  await app.browser.execute(() => document.execCommand("insertText", false, "Thanks Daniel, approved for the budget review."));
  await shot("phone-compose-light");
  await mail.discardCompose(app, "Discard");
  await app.browser.back();
  await app.click(".folders-btn");
  await app.click('.sidebar-head [aria-label="Settings"]');
  await app.waitFor(".settings-nav");
  await shot("phone-settings-light");
  await mail.clickText(app, ".settings-nav-item", "Reading");
  await app.waitFor(".settings-main");
  await shot("phone-settings-reading-light");
  await app.keys("Escape");
});

for (const theme of ["light", "dark"]) {
  test(`tablet, ${theme}`, async () => {
    await app.browser.setWindowSize(820, 1180);
    await setTheme(theme);
    await app.waitUntil(async () => (await app.browser.execute(() => document.querySelector(".app").dataset.layout)) === "tablet", 5000, "tablet layout");
    if (await app.browser.execute(() => document.querySelector(".app").dataset.view === "folders")) await app.browser.back();
    await app.click(".folders-btn");
    await mail.openFolder(app, "Work", "Inbox");
    await mail.openMessage(app, "Q2 launch plan");
    await shot(`tablet-message-${theme}`);
    await app.click(".folders-btn");
    await app.waitFor(".sidebar");
    await shot(`tablet-folders-${theme}`);
    await app.browser.back();
  });
}

test("phone with an iPhone's insets", async () => {
  // The edges iOS reports for a notched iPhone (status bar 47 px, home indicator 34 px), set by
  // hand: the bars reach under them, their content stays clear.
  await app.browser.setWindowSize(390, 844);
  await setTheme("light");
  await app.browser.execute(() => {
    document.documentElement.style.setProperty("--safe-top", "47px");
    document.documentElement.style.setProperty("--safe-bottom", "34px");
  });
  await inbox();
  await mail.openMessage(app, "Q2 launch plan");
  await shot("phone-insets-message-light");
  await app.browser.back();
  await shot("phone-insets-list-light");
});

test("phone with Android's system bars", async () => {
  // A 1080-pixel Android phone drawn edge to edge, with the bars' sizes the shell hands over
  // (SystemBars.kt: status bar 48 px, gesture bar 24 px): the bars of the app reach under them,
  // their content stays clear.
  await app.browser.setWindowSize(412, 915);
  for (const theme of ["dark", "light"]) {
    await setTheme(theme);
    const bars = () =>
      app.browser.execute(() => {
        const s = document.documentElement.style;
        s.setProperty("--sat", "48px");
        s.setProperty("--sab", "24px");
      });
    await bars();
    await inbox();
    await shot(`phone-android-list-${theme}`);
    await app.click(".compose-fab");
    await app.waitFor(".compose");
    await shot(`phone-android-compose-${theme}`);
    await mail.discardCompose(app, "Discard");
  }
});
