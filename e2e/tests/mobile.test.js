// Phones and tablets: the touch layout (ARCALO_MAIL_TOUCH=1 asks the desktop app for it) in a
// window of a phone's size, then a tablet's. One pane at a time and Back walking back, targets
// of at least 44 px, a swipe that archives or marks read, pull to refresh, compose over the whole
// screen, the settings as pages, and the tablet's list and message side by side with the
// folders in a drawer. The mouse plays the finger (pointer events), as the WebDriver has no touch.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, subjects, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

before(async () => {
  server = await startServer();
  const now = Math.floor(Date.now() / 1000);
  for (const [i, subject] of ["Eins", "Zwei", "Drei", "Vier"].entries()) {
    await server.deliver(message({ subject, body: `Nachricht ${subject}` }), { date: now - 600 + i * 60 });
  }
  app = await launch({ width: 390, height: 844, env: { ARCALO_MAIL_TOUCH: "1" } });
  await addAccount(app, server);
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready");
  await waitRow(app, "Vier");
});

after(async () => {
  await app?.close();
  server?.stop();
});

const shown = async (sel) => {
  const all = await app.$$(sel);
  for (const e of all) if (await e.isDisplayed()) return true;
  return false;
};

const rect = (sel) => app.browser.execute((s) => document.querySelector(s)?.getBoundingClientRect().toJSON() ?? null, sel);

async function rowOf(subject) {
  for (const r of await app.$$(".row")) if ((await app.textOf(r)).includes(subject)) return r;
  throw new Error(`no row ${subject}`);
}

/** Drags with the left button from the middle of `el` by (dx, dy). */
async function drag(el, dx, dy) {
  await app.browser
    .action("pointer", { parameters: { pointerType: "mouse" } })
    .move({ origin: el, x: 0, y: 0 })
    .down({ button: 0 })
    .move({ origin: "pointer", x: Math.round(dx / 3), y: Math.round(dy / 3), duration: 80 })
    .move({ origin: "pointer", x: Math.round(dx / 3), y: Math.round(dy / 3), duration: 80 })
    .move({ origin: "pointer", x: dx - 2 * Math.round(dx / 3), y: dy - 2 * Math.round(dy / 3), duration: 80 })
    .up({ button: 0 })
    .perform();
}

it("shows one pane at a time on a phone, and Back walks back", async () => {
  assert.equal(await app.browser.execute(() => document.querySelector(".app").dataset.layout), "phone");
  assert.ok(await shown(".list-pane"));
  assert.ok(!(await shown(".reading-pane")));
  await openMessage(app, "Drei");
  assert.ok(await shown(".reading-pane"));
  assert.ok(!(await shown(".list-pane")));
  // Android's Back: the WebView's history.
  await app.browser.back();
  await app.waitUntil(async () => (await shown(".list-pane")) && !(await shown(".reading-pane")), 5000, "back to the list");
  await app.click(".folders-btn");
  await app.waitUntil(() => shown(".sidebar"), 5000, "folders");
  await clickText(app, ".folder-name", "Posteingang");
  await app.waitUntil(() => shown(".list-pane"), 5000, "the inbox again");
});

it("gives every action a target of at least 44 px", async () => {
  await openMessage(app, "Drei");
  const sizes = await app.browser.execute(() =>
    [...document.querySelectorAll(".reading-pane .toolbar .icon-btn, .touch-actions .icon-btn")]
      .filter((b) => b.offsetParent)
      .map((b) => [b.getAttribute("aria-label"), b.getBoundingClientRect().width, b.getBoundingClientRect().height]),
  );
  assert.ok(sizes.length >= 8, JSON.stringify(sizes));
  for (const [label, w, h] of sizes) assert.ok(w >= 44 && h >= 44, `${label}: ${w}×${h}`);
  const bar = await rect(".touch-actions");
  assert.ok(bar.bottom <= 844 && bar.bottom > 844 - 80, `the bar sits at the bottom: ${JSON.stringify(bar)}`);
  await app.browser.back();
  await app.waitUntil(() => shown(".list-pane"), 5000, "list");
  const fab = await rect(".compose-fab");
  assert.ok(fab.height >= 44, JSON.stringify(fab));
});

it("archives with a swipe to the left and marks read with one to the right", async () => {
  await drag(await rowOf("Eins"), -170, 0);
  await waitRow(app, "Eins", false);
  await waitOnServer(app, server, "Archive", "Eins");
  const zwei = await rowOf("Zwei");
  const unread = async () => app.browser.execute((s) => [...document.querySelectorAll(".row")].find((r) => r.textContent.includes(s))?.dataset.unread === "true", "Zwei");
  const before = await unread();
  await drag(zwei, 170, 0);
  await app.waitUntil(async () => (await unread()) !== before, 5000, "read state flipped");
  // A short drag springs back and does nothing.
  await drag(await rowOf("Vier"), -40, 0);
  await sleep(400);
  assert.ok((await subjects(app)).includes("Vier"));
  assert.ok(!(await shown(".reading-pane")), "a drag is not a tap");
});

it("checks for mail when the list is pulled down", async () => {
  await server.deliver(message({ subject: "Frisch", body: "Gerade angekommen" }));
  await drag(await rowOf("Vier"), 0, 260);
  await waitRow(app, "Frisch", true, 15000);
});

it("writes over the whole screen and opens the settings as pages", async () => {
  await app.click(".compose-fab");
  await app.waitFor(".compose");
  const sheet = await rect(".compose");
  assert.equal(Math.round(sheet.width), 390);
  assert.ok(sheet.height >= 800, JSON.stringify(sheet));
  await app.browser.back();
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 5000, "compose closed by Back");

  await app.click(".folders-btn");
  await app.click('.sidebar-head [aria-label="Einstellungen"]');
  await app.waitFor(".settings-nav");
  assert.ok(!(await shown(".settings-main")), "the sections first");
  await clickText(app, ".settings-nav-item", "Lesen");
  await app.waitFor(".settings-main");
  assert.ok(!(await shown(".settings-nav")));
  assert.ok(await shown('select[aria-label="Nach links wischen"]'), "the swipe settings");
  await app.browser.back();
  await app.waitFor(".settings-nav");
  await app.browser.back();
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
  await clickText(app, ".folder-name", "Posteingang");
  await app.waitUntil(() => shown(".list-pane"), 5000, "the inbox again");
});

it("shows the list and the message side by side on a tablet, the folders in a drawer", async () => {
  await app.browser.setWindowSize(820, 1180);
  await app.waitUntil(async () => (await app.browser.execute(() => document.querySelector(".app").dataset.layout)) === "tablet", 5000, "tablet layout");
  await openMessage(app, "Drei");
  assert.ok((await shown(".list-pane")) && (await shown(".reading-pane")));
  assert.ok(!(await shown(".sidebar")));
  await app.click(".folders-btn");
  await app.waitUntil(() => shown(".sidebar"), 5000, "drawer open");
  // A tap on the dimmed list beside the drawer.
  await app.browser
    .action("pointer", { parameters: { pointerType: "mouse" } })
    .move({ origin: await app.$(".drawer-scrim"), x: 300, y: 0 })
    .down({ button: 0 })
    .up({ button: 0 })
    .perform();
  await app.waitUntil(async () => !(await shown(".sidebar")), 5000, "drawer closed");
});
