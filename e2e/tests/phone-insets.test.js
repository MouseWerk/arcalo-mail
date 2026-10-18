// Android draws the app edge to edge: the status bar (here 48 px), the navigation bar (24 px)
// and a cutout lie over the page, and the shell hands their sizes over as --sat/--sab/--sal/--sar
// (SystemBars.kt). Set by hand here, at the width of a 1080-pixel phone (412 px): the setup
// fills the screen and starts below the status bar, and on every screen the bars at the top and
// the bottom keep their buttons clear of the system bars, menus come up as sheets from the
// bottom edge, and nothing is wider than the screen. The mouse plays the finger.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { openMessage, waitRow } from "../lib/mail.js";

const W = 412;
const H = 915;
const TOP = 48;
const BOTTOM = 24;

let app;
let server;
const it = guarded(test, () => app);

async function insets() {
  await app.browser.execute(
    (t, b) => {
      const s = document.documentElement.style;
      s.setProperty("--sat", `${t}px`);
      s.setProperty("--sab", `${b}px`);
    },
    TOP,
    BOTTOM,
  );
  await new Promise((r) => setTimeout(r, 150));
}

/** The visible buttons, fields and links inside `sel`, as [label, top, bottom, left, right]. */
const targets = (sel) =>
  app.browser.execute((s) => {
    const out = [];
    for (const root of document.querySelectorAll(s))
      for (const e of root.querySelectorAll("button, input, select, textarea, a[href], [role=tab]")) {
        const r = e.getBoundingClientRect();
        const cs = getComputedStyle(e);
        if (r.width === 0 || r.height === 0 || cs.visibility === "hidden") continue;
        // In a row that scrolls sideways (the tabs) a target may lie beyond the edge.
        let sideways = false;
        for (let p = e.parentElement; p && p !== root.parentElement; p = p.parentElement) {
          if (p.scrollWidth > p.clientWidth && /auto|scroll/.test(getComputedStyle(p).overflowX)) sideways = true;
        }
        out.push([e.getAttribute("aria-label") || e.textContent.trim().slice(0, 30) || e.className, r.top, r.bottom, sideways ? 0 : r.left, sideways ? 0 : r.right]);
      }
    return out;
  }, sel);

async function clearOfBars(sel, { top = true, bottom = true } = {}) {
  const list = await targets(sel);
  assert.ok(list.length > 0, `nothing in ${sel}`);
  for (const [label, t, b, l, r] of list) {
    if (top) assert.ok(t >= TOP, `${sel}: „${label}“ under the status bar (top ${t})`);
    if (bottom) assert.ok(b <= H - BOTTOM, `${sel}: „${label}“ under the navigation bar (bottom ${b})`);
    assert.ok(l >= 0 && r <= W + 0.5, `${sel}: „${label}“ outside the screen (${l}–${r})`);
  }
}

async function noSideways() {
  const [doc, app_] = await app.browser.execute(() => [document.documentElement.scrollWidth, document.querySelector(".app")?.scrollWidth ?? 0]);
  assert.ok(doc <= W && app_ <= W, `wider than the screen: ${doc}, ${app_}`);
}

before(async () => {
  server = await startServer();
  const now = Math.floor(Date.now() / 1000);
  for (const [i, subject] of ["Eins", "Zwei", "Drei"].entries()) {
    await server.deliver(message({ subject, body: `Nachricht ${subject}` }), { date: now - 600 + i * 60 });
  }
  app = await launch({ width: W, height: H, env: { ARCALO_MAIL_TOUCH: "1" } });
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("shows the setup over the whole screen, below the status bar", async () => {
  await app.waitFor(".setup-welcome");
  await insets();
  const card = await app.browser.execute(() => document.querySelector(".setup-card").getBoundingClientRect().toJSON());
  assert.equal(Math.round(card.left), 0);
  assert.equal(Math.round(card.width), W);
  const logo = await app.browser.execute(() => document.querySelector(".setup-logo").getBoundingClientRect().top);
  assert.ok(logo >= TOP, `the mark under the status bar: ${logo}`);
  // The top of a setup taller than the screen can be scrolled to (a centered grid cut it off).
  await app.browser.setWindowSize(W, 560);
  await app.browser.execute(() => document.querySelector(".setup").scrollTo(0, 0));
  const h1 = await app.browser.execute(() => document.querySelector(".setup-intro h1").getBoundingClientRect().top);
  assert.ok(h1 >= TOP, `the title is reachable: ${h1}`);
  await app.browser.setWindowSize(W, H);
  await noSideways();
  // No Microsoft sign-in without a client id in the build; „Weiter“ disabled but readable.
  assert.equal((await app.$$(".setup-ms")).length, 0);
  const next = await app.browser.execute(() => {
    const b = document.querySelector(".setup-fields button[type=submit]");
    return { disabled: b.disabled, opacity: getComputedStyle(b).opacity };
  });
  assert.deepEqual(next, { disabled: true, opacity: "1" });
});

it("keeps the list's bars and the compose button clear of the system bars", async () => {
  await addAccount(app, server);
  await app.reload();
  await waitRow(app, "Drei");
  await insets();
  await clearOfBars(".list-head");
  const fab = await app.browser.execute(() => document.querySelector(".compose-fab").getBoundingClientRect().toJSON());
  assert.ok(fab.bottom <= H - BOTTOM && Math.round(fab.height) >= 44, JSON.stringify(fab));
  // Every tab of the sorted inbox, with its name, and Focus.
  const tabs = await app.browser.execute(() => [...document.querySelectorAll(".inbox-tab")].map((b) => b.textContent.trim()));
  assert.equal(tabs.length, 6, JSON.stringify(tabs));
  assert.ok((await app.$$(".row .row-avatar")).length >= 3);
  await noSideways();
});

it("keeps the drawer, the message and its actions clear of the system bars", async () => {
  await app.click(".folders-btn");
  await app.waitFor(".sidebar");
  await clearOfBars(".sidebar-head", { bottom: false });
  await app.browser.back();
  await openMessage(app, "Drei");
  await clearOfBars(".reading-pane .toolbar", { bottom: false });
  await clearOfBars(".touch-actions", { top: false });
  const bar = await app.browser.execute(() => document.querySelector(".touch-actions").getBoundingClientRect().bottom);
  assert.equal(Math.round(bar), H, "the bar reaches under the navigation bar");
  await noSideways();
  // A menu is a sheet from the bottom edge; Back closes it and stays on the message.
  await app.click('.reading-pane [data-action="snooze"]');
  await app.waitFor(".menu-sheet");
  await new Promise((r) => setTimeout(r, 400));
  const sheet = await app.browser.execute(() => document.querySelector(".menu-sheet").getBoundingClientRect().toJSON());
  assert.ok(Math.round(sheet.bottom) === H && Math.round(sheet.width) === W, JSON.stringify(sheet));
  await clearOfBars(".menu-sheet", { top: false });
  await app.browser.back();
  await app.waitUntil(async () => (await app.$$(".menu-sheet")).length === 0, 5000, "the sheet closed");
  assert.ok(await app.browser.execute(() => !!document.querySelector(".reading-pane")?.offsetParent), "still on the message");
  await app.browser.back();
  await app.waitFor(".list-pane");
});

it("writes with an app bar below the status bar", async () => {
  await app.click(".compose-fab");
  await app.waitFor(".compose-bar");
  await clearOfBars(".compose-bar");
  assert.equal((await app.$$(".compose-foot")).length, 0, "no desktop footer on a phone");
  const send = await app.browser.execute(() => document.querySelector(".compose-send").getBoundingClientRect().toJSON());
  assert.ok(Math.round(send.width) >= 44 && Math.round(send.height) >= 44, JSON.stringify(send));
  await app.click(".compose [data-compose='more']");
  await app.waitFor(".menu-sheet");
  await new Promise((r) => setTimeout(r, 400));
  await clearOfBars(".menu-sheet", { top: false });
  await app.browser.back();
  await app.waitUntil(async () => (await app.$$(".menu-sheet")).length === 0, 5000, "the sheet closed");
  assert.equal((await app.$$(".compose")).length, 1, "Back closed the sheet, not the message");
  await app.browser.back();
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
});

it("opens the settings below the status bar", async () => {
  await app.click(".folders-btn");
  await app.click('.sidebar-head [aria-label="Einstellungen"]');
  await app.waitFor(".settings-nav");
  await clearOfBars(".settings-nav", { bottom: false });
  await noSideways();
  await app.keys("Escape");
});

it("keeps the sync line's spinner a small ring", async () => {
  if (!(await app.browser.execute(() => !!document.querySelector(".sidebar")?.offsetParent))) await app.click(".folders-btn");
  await app.waitFor(".sidebar");
  // The line of an account that syncs, as the sidebar draws it.
  const size = await app.browser.execute(() => {
    const line = document.createElement("div");
    line.className = "account-status";
    line.innerHTML =
      '<svg class="spinner" viewBox="0 0 16 16" width="16" height="16"><circle class="spinner-track" cx="8" cy="8" r="7"></circle><path class="spinner-arc" d="M8 1a7 7 0 0 1 7 7"></path></svg><span class="account-status-text">Synchronisiere…</span>';
    document.querySelector(".sidebar .account").prepend(line);
    const r = line.querySelector(".spinner").getBoundingClientRect();
    const text = line.querySelector(".account-status-text").getBoundingClientRect();
    line.remove();
    return { w: r.width, h: r.height, gap: text.left - r.right };
  });
  assert.ok(size.w <= 16 && Math.abs(size.w - size.h) < 0.5, JSON.stringify(size));
  assert.ok(size.gap > 0 && size.gap <= 12, `the text right after it: ${JSON.stringify(size)}`);
});
