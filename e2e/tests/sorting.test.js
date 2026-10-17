// The inbox that sorts itself: tabs from the headers (people, newsletters, notifications,
// receipts, other) with their counts, newsletters bundled per sender and opened as a stream, a
// sender moved to another tab, trackers removed without a request to them, and one-click
// unsubscribe (RFC 8058) to the test server's web side.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import * as mail from "../lib/mail.js";
import { clickText, menuItem, openMessage, openTab, subjects, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);

before(async () => {
  server = await startServer();
  const deliver = (m, ago) => server.deliver(message(m), { date: now - ago });
  const list = [
    "List-Id: Field Notes <letters.fieldnotes.example>",
    `List-Unsubscribe: <${server.webUrl("/unsubscribe?list=fieldnotes")}>, <mailto:leave@fieldnotes.example>`,
    "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
  ];
  await deliver({ from: "Field Notes <letters@fieldnotes.example>", subject: "Field Notes #47", body: "Issue 47", headers: list }, 600);
  await deliver(
    {
      from: "Field Notes <letters@fieldnotes.example>",
      subject: "Field Notes #48",
      body: "Issue 48",
      headers: list,
      html:
        "<h1>Designing for slow software</h1><p>An essay.</p>" +
        `<img src="${server.webUrl("/track/open.php?id=48")}" width="1" height="1">` +
        `<img src="${server.webUrl("/img/hero.png")}" width="600" height="300" alt="Hero">` +
        '<p><a href="https://fieldnotes.example/essay?utm_source=newsletter&amp;utm_medium=email&amp;id=48">Read the essay</a></p>',
    },
    500,
  );
  await deliver({ from: "Bank <noreply@bank.example>", subject: "Neue Anmeldung", body: "Jemand hat sich angemeldet." }, 400);
  await deliver({ from: "Laden <shop@laden.example>", subject: "Deine Bestellung 4417", body: "Danke für deine Bestellung." }, 300);
  await deliver({ from: "Lea <lea@example.org>", subject: "Re: Release 1.0", body: "Sieht gut aus.", headers: ["List-Id: <dev.lists.example>", "List-Post: <mailto:dev@lists.example>"] }, 200);
  await deliver({ subject: "Kaffee morgen?", body: "Um zehn?" }, 100);
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

const tabCount = (name) => mail.tabCount(app, name);

it("sorts the inbox into tabs with their unread counts", async () => {
  await waitRow(app, "Kaffee morgen?");
  await app.waitUntil(async () => (await tabCount("newsletters")) === 2, 15000, "the newsletters' count");
  assert.deepEqual(await subjects(app), ["Kaffee morgen?"], "people only");
  assert.equal(await tabCount("notifications"), 1);
  assert.equal(await tabCount("receipts"), 1);
  assert.equal(await tabCount("other"), 1);
  await app.shot("sorting-tabs");
  await openTab(app, "receipts");
  await waitRow(app, "Deine Bestellung 4417");
  await openTab(app, "other");
  await waitRow(app, "Re: Release 1.0");
});

it("bundles a newsletter's issues and opens them as a stream", async () => {
  await openTab(app, "newsletters");
  await waitRow(app, "Field Notes #48");
  assert.deepEqual(await subjects(app), ["Field Notes #48"], "one row per sender");
  assert.equal(await app.text(".row .thread-count"), "2");
  await (await app.$(".row")).click();
  await app.waitText(".message-subject", /2 Ausgaben von Field Notes/);
  await app.waitUntil(async () => (await app.$$(".stream-subject")).length === 2, 10000, "both issues open");
});

it("removes trackers and never asks them", async () => {
  await app.waitText(".tracker-note", /2 Tracker entfernt/);
  const frame = (await app.$$(".mail-frame"))[0];
  await app.browser.switchFrame(frame);
  const html = await app.browser.execute(() => document.body.innerHTML);
  await app.browser.switchFrame(null);
  assert.doesNotMatch(html, /track\/open/, "no pixel in the frame");
  assert.match(html, /href="https:\/\/fieldnotes\.example\/essay\?id=48"/, "the link without its campaign");
  // Even with remote images allowed, the pixel stays out.
  await clickText(app, ".reading-pane .notice .btn", "Bilder laden");
  await new Promise((r) => setTimeout(r, 1500));
  assert.deepEqual((await server.web()).filter((w) => w.path.includes("/track")), [], "no request to the tracker");
});

it("unsubscribes with one click", async () => {
  await clickText(app, ".unsubscribe-btn span", "Abbestellen");
  await app.waitText(".dialog-title", /Field Notes abbestellen\?/);
  assert.match(await app.text(".dialog-text"), /ein Klick nach RFC 8058/);
  await clickText(app, ".dialog-foot .btn-primary", "Abbestellen");
  await app.waitText(".toast-title", /Field Notes abbestellt/);
  const posts = (await server.web()).filter((w) => w.path.startsWith("/unsubscribe"));
  assert.equal(posts.length, 1, JSON.stringify(posts));
  assert.equal(posts[0].method, "POST");
  assert.equal(posts[0].path, "/unsubscribe?list=fieldnotes");
  assert.equal(posts[0].body, "List-Unsubscribe=One-Click");
  assert.match(posts[0].content_type, /application\/x-www-form-urlencoded/);
  assert.equal((await server.sent()).length, 0, "no e-mail when one click works");
  // The subscriptions list says so.
  await clickText(app, ".list-title-tools .btn span", "Abos");
  await app.waitText(".subscription-item", /Field Notes/);
  assert.match(await app.text(".subscription-item"), /Abbestellt/);
  await app.shot("sorting-subscriptions");
});

it("moves a sender to another tab, for good", async () => {
  await (await app.$(".sidebar")).click();
  await clickText(app, ".account .folder-name", "Posteingang");
  await openTab(app, "notifications");
  await openMessage(app, "Neue Anmeldung");
  await app.click('.reading-pane .toolbar [data-action="more"]');
  await menuItem(app, "Absender einsortieren unter…");
  await menuItem(app, "Personen");
  await app.waitText(".toast-title", /Nachrichten von Bank erscheinen jetzt unter Personen/);
  await waitRow(app, "Neue Anmeldung", false);
  await openTab(app, "people");
  await waitRow(app, "Neue Anmeldung");
  // New mail of the sender goes there too.
  await server.deliver(message({ from: "Bank <noreply@bank.example>", subject: "Kontoauszug", body: "Neu." }));
  await waitRow(app, "Kontoauszug", true, 30000);
});

it("shows people and pinned messages only in focus", async () => {
  await openTab(app, "receipts");
  await openMessage(app, "Deine Bestellung 4417");
  await app.keys("p");
  await app.waitText(".toast-title", /Angeheftet/);
  await app.click(".focus-btn");
  await app.waitFor(".focus-note");
  await waitRow(app, "Deine Bestellung 4417");
  const list = await subjects(app);
  assert.equal(list[0], "Deine Bestellung 4417", "pinned first");
  assert.ok(list.includes("Kaffee morgen?") && !list.includes("Re: Release 1.0"), list.join(", "));
  await clickText(app, ".focus-note .link-btn", "Fokus beenden");
  await app.waitFor(".inbox-tabs");
});
