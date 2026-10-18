// Three accounts: the unified inbox opens first, the switcher in the list's header shows each
// account with its unread mail and narrows the list to one, „Alle gesendeten“ holds the Sent
// folders of all three, a search runs over every account and the switcher narrows it, a new
// message is written from the account in view, and the next start opens the view of the last.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { openView, subjects, waitRow } from "../lib/mail.js";

let app;
const servers = [];
const ids = [];
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);
const NAMES = ["Arbeit", "Privat", "Verein"];

before(async () => {
  for (const [i, name] of NAMES.entries()) {
    const user = `maria@${name.toLowerCase()}.example`;
    const s = await startServer({ user });
    servers.push(s);
    for (let k = 0; k <= i; k++) {
      const at = now - 600 + i * 60 + k;
      await s.deliver(message({ to: user, subject: `${name}: Post ${k + 1}`, body: `Gemeinsames Stichwort Zugfahrt ${name}.`, date: new Date(at * 1000) }), { date: at });
    }
    const sentAt = now - 900 + i * 60;
    await s.deliver(message({ from: user, to: "lea@example.org", subject: `${name}: gesendet`, body: "Gesendet.", date: new Date(sentAt * 1000) }), { mailbox: "Sent", seen: true, date: sentAt });
  }
  app = await launch();
  for (const [i, s] of servers.entries()) ids.push((await addAccount(app, s, { name: NAMES[i] })).id);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  for (const s of servers) s.stop();
});

const chips = () =>
  app.browser.execute(() =>
    [...document.querySelectorAll(".acct-switch .acct-chip")].map((c) => [c.dataset.account, c.querySelector(".acct-chip-name").textContent, c.querySelector(".acct-chip-count")?.textContent ?? "", c.getAttribute("aria-selected")]),
  );

it("opens the unified inbox with a switcher of every account and its unread mail", async () => {
  await waitRow(app, "Verein: Post 3", true, 30000);
  await waitRow(app, "Arbeit: Post 1", true, 30000);
  assert.equal(await app.text(".list-title h1"), "Alle Posteingänge");
  assert.equal((await subjects(app)).length, 6);
  await app.waitUntil(async () => (await chips()).map((c) => c[2]).join(",") === "6,1,2,3", 15000, "unread counts");
  const c = await chips();
  assert.deepEqual(
    c.map(([id, name, , selected]) => [id, name, selected]),
    [
      ["all", "Alle", "true"],
      [String(ids[0]), "Arbeit", "false"],
      [String(ids[1]), "Privat", "false"],
      [String(ids[2]), "Verein", "false"],
    ],
  );
  // Every row of the unified inbox carries its account's dot.
  const dots = await app.browser.execute(() => document.querySelectorAll(".row .row-account").length);
  assert.equal(dots, 6);
});

it("narrows the list to one account and back to all with one tap", async () => {
  await app.click(`.acct-chip[data-account="${ids[2]}"]`);
  await app.waitUntil(async () => (await subjects(app)).length === 3, 10000, "Verein's inbox");
  assert.ok((await subjects(app)).every((s) => s.startsWith("Verein")));
  assert.equal(await app.text(".list-title h1"), "Posteingang");
  await app.click('.acct-chip[data-account="all"]');
  await app.waitUntil(async () => (await subjects(app)).length === 6, 10000, "all inboxes again");
});

it("shows the Sent folders of every account together", async () => {
  await openView(app, "Alle gesendeten");
  await app.waitUntil(async () => (await subjects(app)).length === 3, 15000, "three sent messages");
  assert.deepEqual(await subjects(app), ["Verein: gesendet", "Privat: gesendet", "Arbeit: gesendet"]);
  // The switcher goes to one account's Sent folder.
  await app.click(`.acct-chip[data-account="${ids[1]}"]`);
  await app.waitUntil(async () => (await subjects(app)).join() === "Privat: gesendet", 10000, "Privat's Sent");
  await app.click('.acct-chip[data-account="all"]');
  await app.waitUntil(async () => (await subjects(app)).length === 3, 10000, "all sent again");
});

it("searches every account, and the switcher narrows the results", async () => {
  await app.fill(".search-input", "Zugfahrt");
  await app.waitUntil(async () => (await subjects(app)).length === 6, 15000, "results of all accounts");
  await app.click(`.acct-chip[data-account="${ids[0]}"]`);
  await app.waitUntil(async () => (await subjects(app)).join() === "Arbeit: Post 1", 10000, "Arbeit's result");
  await app.click(".search-clear");
});

it("writes from the account in view", async () => {
  await openView(app, "Alle Posteingänge");
  await app.click(`.acct-chip[data-account="${ids[1]}"]`);
  await app.waitUntil(async () => (await subjects(app)).length === 2, 10000, "Privat's inbox");
  await app.keys("c");
  await app.waitFor(".compose");
  assert.equal(await (await app.$("#compose-from")).getAttribute("data-value"), String(ids[1]));
  await app.keys("Escape");
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
});

it("opens the last view again at the next start", async () => {
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready");
  await app.waitUntil(async () => (await subjects(app)).length === 2, 15000, "Privat's inbox after the start");
  assert.ok((await subjects(app)).every((s) => s.startsWith("Privat")));
});
