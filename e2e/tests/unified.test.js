// The unified inbox: with two accounts it opens first, sorts the mail of both by time with a
// colored dot per account, and its actions reach the right server.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, subjects, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let work;
let home;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);

before(async () => {
  work = await startServer({ user: "maria@work.example" });
  home = await startServer({ user: "maria@home.example" });
  const at = (ago) => ({ date: new Date((now - ago) * 1000) });
  await work.deliver(message({ to: "maria@work.example", subject: "Arbeit: Plan", body: "Plan.", ...at(300) }), { date: now - 300 });
  await home.deliver(message({ from: "Lea <lea@example.org>", to: "maria@home.example", subject: "Privat: Kino", body: "Kino?", ...at(200) }), { date: now - 200 });
  await work.deliver(message({ to: "maria@work.example", subject: "Arbeit: Neu", body: "Neu.", ...at(100) }), { date: now - 100 });
  app = await launch();
  await addAccount(app, work, { name: "Arbeit" });
  await addAccount(app, home, { name: "Privat" });
  await app.reload();
});

after(async () => {
  await app?.close();
  work?.stop();
  home?.stop();
});

it("opens all inboxes first, newest first, with the account's dot", async () => {
  await waitRow(app, "Privat: Kino", true, 30000);
  await waitRow(app, "Arbeit: Plan", true, 30000);
  assert.equal(await app.text(".list-title h1"), "Alle Posteingänge");
  assert.match(await app.text(".views .folder[aria-current='page']"), /Alle Posteingänge/);
  assert.deepEqual(await subjects(app), ["Arbeit: Neu", "Privat: Kino", "Arbeit: Plan"]);
  const dots = await app.browser.execute(() => [...document.querySelectorAll(".row")].map((r) => [r.querySelector(".row-subject").textContent, r.querySelector(".account-dot")?.dataset.color, r.querySelector(".account-dot")?.getAttribute("aria-label")]));
  assert.deepEqual(dots, [
    ["Arbeit: Neu", "1", "Konto: Arbeit"],
    ["Privat: Kino", "2", "Konto: Privat"],
    ["Arbeit: Plan", "1", "Konto: Arbeit"],
  ]);
  assert.match(await app.text(".views .folder[aria-current='page'] .count"), /^3$/);
  await app.shot("unified");
});

it("acts on the right account", async () => {
  await openMessage(app, "Privat: Kino");
  await app.keys("e");
  await waitRow(app, "Privat: Kino", false);
  await waitOnServer(app, home, "Archive", "Privat: Kino");
  assert.ok(!(await work.state()).mailboxes.Archive?.some((m) => m.subject === "Privat: Kino"));
});

it("shows one account's inbox without the dots", async () => {
  await clickText(app, ".account .folder-name", "Posteingang");
  await waitRow(app, "Arbeit: Neu");
  assert.equal((await app.$$(".row .account-dot")).length, 0);
  assert.ok(!(await subjects(app)).some((s) => s.startsWith("Privat")));
});
