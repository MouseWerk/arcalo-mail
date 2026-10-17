// Speed, with budgets: the first rows after a start (cold and warm), scrolling through 10,000
// rows (frame times while the list moves, the rows kept in the page, their heights), opening a
// message, and switching tabs and accounts back and forth (from what was seen a moment ago, no
// spinner). The budgets leave room for a CI machine drawing in software under Xvfb; the numbers
// are printed as „PERF <name> <ms>“.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { openFolder } from "../lib/mail.js";

const ROWS = 10000;
const BUDGET = {
  coldFirstRows: 5000,
  warmFirstRows: 3000,
  frameP95: 120,
  frameMean: 50,
  openMessage: 1500,
  switchBack: 250,
};

let app;
let big;
let small;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);
const perf = (name, ms) => console.log(`PERF ${name} ${Math.round(ms)}`);

before(async () => {
  big = await startServer({ user: "maria@big.example" });
  small = await startServer({ user: "maria@small.example" });
  // Ten thousand messages, delivered a few at a time.
  for (let i = 0; i < ROWS; i += 50) {
    await Promise.all(
      Array.from({ length: Math.min(50, ROWS - i) }, (_, k) =>
        big.deliver(message({ from: `Absender ${(i + k) % 97} <a${(i + k) % 97}@example.org>`, to: "maria@big.example", subject: `Nachricht ${i + k}`, body: `Text ${i + k}` }), { date: now - (ROWS - i - k) * 60, seen: true }),
      ),
    );
  }
  for (let i = 0; i < 20; i++) await small.deliver(message({ to: "maria@small.example", subject: `Klein ${i}`, body: "Klein." }), { date: now - i * 60 });
});

after(async () => {
  await app?.close();
  big?.stop();
  small?.stop();
});

/** Milliseconds from the page's start until a row is drawn (polled every 25 ms). */
async function firstRows() {
  await app.browser.waitUntil(async () => (await app.$$(".messages .row[data-id]")).length > 0, { timeout: 60000, interval: 25 });
  return app.browser.execute(() => performance.now());
}

it("draws the first rows soon after a start, cold and warm", async () => {
  app = await launch();
  await addAccount(app, big, { name: "Groß" });
  await addAccount(app, small, { name: "Klein" });
  // Cold: the first start with these accounts (the store fills while the rows come).
  await app.browser.refresh();
  const cold = await firstRows();
  perf("cold-first-rows", cold);
  // Everything synced, then warm starts.
  await app.waitUntil(
    async () => (await app.invoke("list_mailboxes", { accountId: (await app.invoke("list_accounts"))[0].id })).find((m) => m.role === "inbox")?.total >= ROWS,
    240000,
    "the big inbox synced",
  );
  const warm = [];
  for (let i = 0; i < 3; i++) {
    await app.browser.refresh();
    warm.push(await firstRows());
  }
  warm.sort((a, b) => a - b);
  perf("warm-first-rows", warm[1]);
  assert.ok(cold < BUDGET.coldFirstRows, `cold ${cold} ms`);
  assert.ok(warm[1] < BUDGET.warmFirstRows, `warm ${warm[1]} ms`);
});

it("scrolls 10,000 rows with short frames, few rows in the page and steady heights", async () => {
  await openFolder(app, "Groß", "Posteingang");
  await app.waitUntil(async () => Number(await app.browser.execute(() => document.querySelector(".messages-inner")?.style.height.replace("px", "") ?? 0)) > 0, 15000, "the list");
  const r = await app.browser.executeAsync((done) => {
    const el = document.querySelector(".messages");
    const frames = [];
    let last = performance.now();
    let n = 0;
    let maxRows = 0;
    const heights = new Set();
    const step = () => {
      const t = performance.now();
      frames.push(t - last);
      last = t;
      el.scrollTop += 900;
      const rows = el.querySelectorAll(".row");
      maxRows = Math.max(maxRows, rows.length);
      for (const row of rows) heights.add(row.style.height);
      if (++n < 150 && el.scrollTop + el.clientHeight < el.scrollHeight) requestAnimationFrame(step);
      else done({ frames: frames.slice(1), maxRows, heights: [...heights], end: el.scrollTop, height: el.scrollHeight });
    };
    requestAnimationFrame(step);
  });
  const f = [...r.frames].sort((a, b) => a - b);
  const mean = f.reduce((a, b) => a + b, 0) / f.length;
  const p95 = f[Math.floor(f.length * 0.95)];
  perf("scroll-frame-mean", mean);
  perf("scroll-frame-p95", p95);
  perf("scroll-rows-in-page", r.maxRows);
  assert.ok(r.maxRows < 80, `rows in the page: ${r.maxRows}`);
  assert.equal(r.heights.length, 1, `row heights: ${r.heights}`);
  assert.ok(mean < BUDGET.frameMean, `mean frame ${mean} ms`);
  assert.ok(p95 < BUDGET.frameP95, `p95 frame ${p95} ms`);
});

it("opens a message quickly", async () => {
  await app.browser.execute(() => {
    document.querySelector(".messages").scrollTop = 0;
  });
  await app.waitUntil(async () => (await app.$$(".row[data-id]")).length > 3, 10000, "rows");
  const rows = await app.$$(".row[data-id]");
  const subject = await app.textOf(await rows[2].$(".row-subject"));
  const t0 = await app.browser.execute(() => performance.now());
  await rows[2].click();
  await app.browser.waitUntil(async () => (await app.browser.execute((s) => document.querySelector(".message-subject")?.textContent?.includes(s), subject)) === true, { timeout: 10000, interval: 10 });
  const ms = (await app.browser.execute(() => performance.now())) - t0;
  perf("open-message", ms);
  assert.ok(ms < BUDGET.openMessage, `open ${ms} ms`);
});

/** Milliseconds from a click on `sel` until the list shows rows of `key` (a subject's start). */
async function switchTo(sel, key) {
  const t0 = await app.browser.execute(() => performance.now());
  await app.click(sel);
  await app.browser.waitUntil(async () => (await app.browser.execute((k) => [...document.querySelectorAll(".row[data-id] .row-subject")].some((x) => x.textContent.includes(k)), key)) === true, { timeout: 10000, interval: 5 });
  const ms = (await app.browser.execute(() => performance.now())) - t0;
  const spinner = await app.browser.execute(() => !!document.querySelector(".messages .spinner"));
  return { ms, spinner };
}

/** Marks the inbox of the account `name` in the folders, for a click without a search. */
const markInbox = (name) =>
  app.browser.execute((n) => {
    for (const x of document.querySelectorAll("[data-perf]")) x.removeAttribute("data-perf");
    const section = [...document.querySelectorAll(".sidebar .account")].find((a) => a.querySelector(".account-head")?.textContent?.trim().toLowerCase() === n.toLowerCase());
    section?.querySelector('.folder[data-role="inbox"]')?.setAttribute("data-perf", "");
  }, name);

it("switches accounts back and forth without waiting", async () => {
  // The first visit of each loads; the way back comes from what was seen.
  await markInbox("Klein");
  await switchTo(".folder[data-perf]", "Klein");
  await markInbox("Groß");
  await switchTo(".folder[data-perf]", "Nachricht");
  await markInbox("Klein");
  const back = await switchTo(".folder[data-perf]", "Klein");
  perf("switch-account-back", back.ms);
  await markInbox("Groß");
  const forth = await switchTo(".folder[data-perf]", "Nachricht");
  perf("switch-account-forth", forth.ms);
  assert.ok(back.ms < BUDGET.switchBack && forth.ms < BUDGET.switchBack, `switch ${back.ms} / ${forth.ms} ms`);
  assert.equal(back.spinner || forth.spinner, false, "no spinner when switching back");
});
