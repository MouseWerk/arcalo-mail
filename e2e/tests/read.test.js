// Reading: the list fills from the server, an HTML message with its remote images blocked until
// „Bilder laden“, plain text with the quote folded, attachments, keys (j/k, e, #, s, u), a change
// made by another client, and new mail arriving through IDLE.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, subjects, waitOnServer, waitRow } from "../lib/mail.js";
import { pdf } from "../lib/png.js";

let app;
let server;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);

before(async () => {
  server = await startServer();
  const deliver = (m, opts = {}) => server.deliver(message(m), { date: now - (opts.ago ?? 0), ...opts.flags });
  await deliver(
    {
      from: "Rundbrief <post@verein.example>",
      subject: "Neuigkeiten aus dem Verein",
      body: "Neuigkeiten aus dem Verein (Textfassung)",
      html: '<h1>Neuigkeiten</h1><p>Das Sommerfest ist am <b>12. Juli</b>.</p><img src="https://bilder.verein.example/fest.jpg" alt="Fest"><a href="https://verein.example/fest">Mehr lesen</a>',
    },
    { ago: 300 },
  );
  await deliver(
    {
      from: "Lea Sommer <lea@example.org>",
      subject: "Rechnung März",
      body: "Hallo Maria,\n\nanbei die Rechnung.\n\nLea\n\nAm 1. März schrieb Maria:\n> Schickst du mir die Rechnung?\n> Danke!",
      attachments: [{ name: "rechnung-maerz.pdf", type: "application/pdf", data: pdf("Rechnung") }],
    },
    { ago: 200 },
  );
  await deliver({ subject: "Kaffee morgen?", body: "Um zehn in der Küche?" }, { ago: 100 });
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("lists the inbox, newest first, with the unread count", async () => {
  await waitRow(app, "Neuigkeiten aus dem Verein");
  assert.deepEqual(await subjects(app), ["Kaffee morgen?", "Rechnung März", "Neuigkeiten aus dem Verein"]);
  assert.match(await app.text(".folder[aria-current] .count"), /^3$/);
});

it("blocks remote images until they are loaded", async () => {
  await openMessage(app, "Neuigkeiten aus dem Verein");
  assert.match(await app.text(".reading-pane .notice"), /Ein externes Bild wurde blockiert/);
  const frame = await app.$(".mail-frame");
  await app.browser.switchFrame(frame);
  assert.equal(await app.browser.execute(() => document.querySelector("img")?.getAttribute("src") ?? null), null, "no remote address in the frame");
  assert.match(await app.browser.execute(() => document.body.innerText), /Sommerfest/);
  await app.browser.switchFrame(null);
  await app.shot("read-html-blocked");
  await clickText(app, ".reading-pane .notice .btn", "Bilder laden");
  await app.waitUntil(async () => (await app.$$(".reading-pane .notice")).length === 0, 8000, "notice gone");
  await app.browser.switchFrame(await app.$(".mail-frame"));
  assert.equal(await app.browser.execute(() => document.querySelector("img")?.getAttribute("src")), "https://bilder.verein.example/fest.jpg");
  await app.browser.switchFrame(null);
  // Opening marked it read, here and on the server.
  await waitOnServer(app, server, "INBOX", "Neuigkeiten aus dem Verein", { where: (m) => m.seen });
});

it("folds the quote and lists attachments", async () => {
  await openMessage(app, "Rechnung März");
  await app.browser.switchFrame(await app.$(".mail-frame"));
  const text = await app.browser.execute(() => document.body.innerText);
  await app.browser.switchFrame(null);
  assert.match(text, /anbei die Rechnung/);
  assert.doesNotMatch(text, /Schickst du/, "the quote is folded");
  assert.match(await app.text(".attachment-name"), /rechnung-maerz\.pdf/);
  await app.click(".attachment-main");
  await app.waitUntil(async () => app.opened().some((l) => l.endsWith("rechnung-maerz.pdf")), 10000, "attachment opened");
  await clickText(app, ".message-body-tools .btn", "Zitierten Text anzeigen");
  await app.browser.switchFrame(await app.$(".mail-frame"));
  assert.match(await app.browser.execute(() => document.body.innerText), /Schickst du/);
  await app.browser.switchFrame(null);
  await app.shot("read-attachment");
});

it("moves through the list and acts with keys", async () => {
  await (await app.$(".messages")).click();
  await openMessage(app, "Kaffee morgen?");
  await app.keys("j");
  await app.waitText(".message-subject", /Rechnung März/);
  await app.keys("k");
  await app.waitText(".message-subject", /Kaffee morgen/);
  // s flags, on the server too.
  await app.keys("s");
  await waitOnServer(app, server, "INBOX", "Kaffee morgen?", { where: (m) => m.flagged });
  // e archives; u brings it back before the server sees it.
  await app.keys("e");
  await waitRow(app, "Kaffee morgen?", false);
  await app.waitText(".toast-title", /Archiviert/);
  await app.keys("u");
  await waitRow(app, "Kaffee morgen?");
  await new Promise((r) => setTimeout(r, 6500));
  await waitOnServer(app, server, "INBOX", "Kaffee morgen?");
  // # deletes: into the trash on the server once the undo time is over.
  await openMessage(app, "Kaffee morgen?");
  await app.keys("#");
  await waitRow(app, "Kaffee morgen?", false);
  await waitOnServer(app, server, "Trash", "Kaffee morgen?");
  await waitOnServer(app, server, "INBOX", "Kaffee morgen?", { present: false });
});

it("follows changes from other clients and new mail", async () => {
  // Another client marks the invoice unread again: the dot comes back.
  const state = await server.state();
  const uid = state.mailboxes.INBOX.find((m) => m.subject === "Rechnung März").uid;
  await server.setFlags("INBOX", uid, { seen: false });
  await (await app.$(".sidebar")).click();
  await app.invoke("sync_now", { accountId: (await app.invoke("list_accounts"))[0].id });
  await app.waitUntil(async () => {
    for (const r of await app.$$(".row")) if ((await app.textOf(r)).includes("Rechnung März")) return (await r.$$(".unread-dot")).length === 1;
    return false;
  }, 20000, "unread again");
  // New mail arrives while the app waits in IDLE.
  await server.deliver(message({ subject: "Gerade eingetroffen", body: "Hallo!" }), { date: Math.floor(Date.now() / 1000) });
  await waitRow(app, "Gerade eingetroffen", true, 30000);
  assert.equal((await subjects(app))[0], "Gerade eingetroffen");
});
