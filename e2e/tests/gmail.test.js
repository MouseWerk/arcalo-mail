// A Gmail account as a real one is (crates/mail-testserver/src/gmail.rs): years of mail since
// 2015, localized [Gmail] folders, labels with brackets, one message in INBOX, „Alle
// Nachrichten“ and a label, receipts and newsletters as shops send them, HTML with pictures
// inside and from the web, and a slow server (each batch of headers waits, old large bodies
// come slowly). What a phone showed with such an account, checked in the app:
// - the newest mail first, and before the first sync is through;
// - first lines for the newest mail while the rest still syncs;
// - an old message that is not here yet opens in a few seconds while the sync goes on;
// - folder names exactly as Gmail has them, none twice („Entwürfe“), no bracket cut off;
// - every message once (the PlayStation receipt is in three folders);
// - receipts and newsletters not under „Personen“;
// - the picture inside a message shown, pictures from the web only when loaded.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, openMessage, openTab, subjects } from "../lib/mail.js";
import { measureNames, nameProblems } from "../lib/geometry.js";

const USER = "maria.muster@gmail.com";
const ALL_MAIL = "[Gmail]/Alle Nachrichten";
/** How long opening an old message may take while the sync runs (the server needs 1.5 s). */
const OPEN_BUDGET_MS = 6000;
/** How long the newest rows may wait for their first lines. */
const SNIPPET_BUDGET_MS = 15000;

let app;
let server;
let fixture;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const done = (served, folder) => served[folder] && served[folder].distinct >= served[folder].total;

before(async () => {
  server = await startServer({ user: USER, password: "app-passwort" });
  fixture = await server.gmail(3000);
  await server.slow({ headers: 2000, oldBodies: 1500 });
  app = await launch();
  await addAccount(app, server, { sender: "Maria Muster" });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready");
});

after(async () => {
  await app?.close();
  server?.stop();
});

/** The rows of the list as the user sees them: subject, sender, date and first line. */
const rows = () =>
  app.browser.execute(() =>
    [...document.querySelectorAll(".row:not(.row-placeholder)")]
      .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top)
      .map((r) => ({
        subject: r.querySelector(".row-subject")?.textContent.trim() ?? "",
        from: r.querySelector(".row-from")?.textContent.trim() ?? "",
        date: r.querySelector(".row-date")?.textContent.trim() ?? "",
        snippet: r.querySelector(".row-snippet")?.textContent.trim() ?? "",
      })),
  );

it("shows the newest mail first, before the first sync is through", async () => {
  const newest = fixture.newest[0];
  let first = null;
  let firstServed = null;
  let seenNewest = null;
  const start = Date.now();
  while (Date.now() - start < 90000) {
    const list = await rows();
    if (list.length && !first) {
      first = list.slice(0, 5).map((r) => r.subject);
      firstServed = (await server.served()).INBOX;
    }
    if (list.some((r) => r.subject === newest)) {
      seenNewest = (await server.served()).INBOX;
      break;
    }
    await sleep(150);
  }
  assert.ok(first, "the list never showed a row");
  assert.equal(first[0], newest, `the first rows shown were ${JSON.stringify(first)} (after ${firstServed?.distinct} of ${firstServed?.total} headers), not the newest mail`);
  assert.ok(seenNewest, "the newest mail never showed");
  assert.ok(seenNewest.distinct < seenNewest.total, `the newest mail showed only after all ${seenNewest.total} headers of the inbox were fetched`);
});

it("gives the newest mail its first lines while the rest still syncs", async () => {
  const start = Date.now();
  let top = [];
  while (Date.now() - start < SNIPPET_BUDGET_MS) {
    top = (await rows()).slice(0, 8);
    if (top.length >= 8 && top.every((r) => r.snippet)) break;
    await sleep(250);
  }
  const missing = top.filter((r) => !r.snippet).map((r) => r.subject);
  assert.ok(top.length >= 8, `only ${top.length} rows`);
  assert.deepEqual(missing, [], `no first lines after ${SNIPPET_BUDGET_MS / 1000} s for ${JSON.stringify(missing)}`);
});

it("opens an old message quickly while the sync goes on", async () => {
  // The old message is in the inbox once the inbox's headers are here; All Mail is still syncing.
  await app.waitUntil(async () => done(await server.served(), "INBOX"), 90000, "the inbox's headers");
  const before = (await server.served())[ALL_MAIL];
  assert.ok(!done({ [ALL_MAIL]: before }, ALL_MAIL), "All Mail was through before the test could open the message (make the server slower)");
  await app.fill(".search-input", fixture.old_large);
  await app.waitUntil(async () => (await subjects(app)).includes(fixture.old_large), 15000, "the old message found");
  const t0 = Date.now();
  for (const r of await app.$$(".row")) {
    if ((await app.textOf(r)).includes(fixture.old_large)) {
      await r.click();
      break;
    }
  }
  await app.waitText(".message-subject", new RegExp(fixture.old_large), 30000);
  await app.waitUntil(async () => (await app.$$(".message-loading")).length === 0, 60000, "the old message loaded");
  const took = Date.now() - t0;
  const during = (await server.served())[ALL_MAIL];
  await app.click(".search-clear").catch(() => {});
  assert.ok(took <= OPEN_BUDGET_MS, `opening the old message took ${took} ms (budget ${OPEN_BUDGET_MS} ms) while All Mail synced (${before.distinct} → ${during.distinct} of ${during.total})`);
});

it("lists the Gmail folders by their names, each once and whole", async () => {
  await app.waitUntil(async () => (await app.$$(".account .folder-name")).length >= 8, 30000, "the folders");
  const names = await app.browser.execute(measureNames, ".account .folder-name");
  const problems = nameProblems(names, ["Posteingang", "Entwürfe", "[Notion]", "Inbox", "Belege"]);
  const texts = names.map((n) => n.text);
  const twice = [...new Set(texts.filter((t, i) => texts.indexOf(t) !== i))];
  for (const t of twice) problems.push(`${JSON.stringify(t)} is listed ${texts.filter((x) => x === t).length} times`);
  assert.deepEqual(problems, [], `folders: ${JSON.stringify(texts)}`);
});

it("shows a nested label inside its parent", async () => {
  // „[Notion]/Inbox“ under „[Notion]“, not a folder of its own on the top level.
  const indent = await app.browser.execute(() => {
    const at = (name) => [...document.querySelectorAll(".account .folder")].find((f) => f.querySelector(".folder-name")?.textContent === name);
    const pad = (f) => parseFloat(getComputedStyle(f).paddingInlineStart);
    const parent = at("[Notion]");
    const child = at("Inbox");
    return parent && child ? [pad(parent), pad(child)] : null;
  });
  assert.ok(indent && indent[1] > indent[0], `„Inbox“ is not inside „[Notion]“: ${JSON.stringify(indent)}`);
});

it("shows every message once", async () => {
  await app.waitUntil(async () => {
    const s = await server.served();
    return Object.keys(s).every((k) => !s[k].total || done(s, k));
  }, 120000, "the first sync of every folder");
  const key = (r) => `${r.subject}|${r.from}|${r.date}`;
  const dupes = (list) => list.map(key).filter((k, i, all) => all.indexOf(k) !== i);
  for (const tab of ["people", "newsletters", "notifications", "receipts"]) {
    if ((await app.$$(`.inbox-tab[data-tab="${tab}"]`)).length === 0) continue;
    await openTab(app, tab);
    await sleep(500);
    assert.deepEqual(dupes(await rows()), [], `twice in the tab ${tab}`);
  }
  await app.fill(".search-input", "PlayStation");
  await app.waitUntil(async () => (await subjects(app)).includes(fixture.playstation), 15000, "PlayStation found");
  await sleep(800);
  const found = await rows();
  await app.click(".search-clear").catch(() => {});
  assert.deepEqual(dupes(found), [], `search shows a message twice: ${JSON.stringify(found.map(key))}`);
});

it("keeps receipts and newsletters out of Personen", async () => {
  await openTab(app, "people");
  await sleep(500);
  const people = (await rows()).map((r) => r.subject);
  const wrong = fixture.expected.filter((e) => e.category !== "people" && people.includes(e.subject)).map((e) => `${e.subject} (${e.from}, belongs in ${e.category})`);
  assert.deepEqual(wrong, [], "in Personen");
  await openTab(app, "receipts");
  await app.waitUntil(async () => (await subjects(app)).includes(fixture.playstation), 10000, "the PlayStation receipt under Belege");
});

it("shows the picture inside a message and loads pictures from the web only when asked", async () => {
  await openTab(app, "people");
  await openMessage(app, fixture.cid);
  await app.browser.switchFrame(await app.$(".mail-frame"));
  await app.waitUntil(async () => (await app.browser.execute(() => [...document.images].some((i) => i.complete && i.naturalWidth > 0))) === true, 10000, "the picture inside");
  const pics = await app.browser.execute(() => [...document.images].map((i) => ({ src: i.getAttribute("src")?.slice(0, 30) ?? null, w: i.naturalWidth })));
  await app.browser.switchFrame(null);
  assert.ok(pics.length === 1 && pics[0].w === 24 && /^data:image\/png/.test(pics[0].src), JSON.stringify(pics));

  // The newsletters tab shows one row per sender: the message is opened from search.
  await app.fill(".search-input", fixture.remote);
  await openMessage(app, fixture.remote);
  assert.match(await app.text(".reading-pane .notice"), /blockiert/);
  const frameImages = async () => {
    await app.browser.switchFrame(await app.$(".mail-frame"));
    const srcs = await app.browser.execute(() => [...document.images].map((i) => i.getAttribute("src")));
    await app.browser.switchFrame(null);
    return srcs;
  };
  assert.ok(!(await frameImages()).some((s) => s?.includes("bilder.zeit.example")), "no address from the web in the frame");
  await clickText(app, ".reading-pane .notice .btn", "Bilder laden");
  await app.waitUntil(async () => (await frameImages()).some((s) => s?.startsWith("https://bilder.zeit.example/")), 10000, "the picture from the web in the frame");
  await app.click(".search-clear").catch(() => {});
});
