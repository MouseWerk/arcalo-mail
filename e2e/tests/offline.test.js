// Offline: with the server gone, reading goes on from the local copy, actions apply at once and
// wait in the queue (the outbox in the sidebar), a message waits too; when the server is back,
// everything reaches it in order.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openFolder, openMessage, subjects, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

before(async () => {
  server = await startServer();
  const now = Math.floor(Date.now() / 1000);
  await server.deliver(message({ subject: "Wochenplan", body: "Montag Einkauf, Dienstag Sport." }), { date: now - 300 });
  await server.deliver(message({ subject: "Alte Rechnung", body: "Längst bezahlt." }), { date: now - 200 });
  await server.deliver(message({ subject: "Frage zum Urlaub", body: "Wann fahrt ihr los?" }), { date: now - 100 });
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
  await waitRow(app, "Wochenplan");
  await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 3, 20000, "bodies fetched");
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("keeps working while the server is gone", async () => {
  await server.offline();
  await app.waitText(".account-status", /Keine Verbindung|Offline/, 30000);
  // Reading from the local copy.
  await openMessage(app, "Wochenplan");
  await app.browser.switchFrame(await app.$(".mail-frame"));
  assert.match(await app.browser.execute(() => document.body.innerText), /Dienstag Sport/);
  await app.browser.switchFrame(null);
  // Archive and flag: done here at once.
  await openMessage(app, "Alte Rechnung");
  await app.keys("e");
  await waitRow(app, "Alte Rechnung", false);
  await openMessage(app, "Frage zum Urlaub");
  await app.keys("s");
  // An answer waits in the outbox.
  await app.keys("r");
  await app.waitFor(".compose");
  await app.type("Am Samstag früh.");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "sheet closed");
  await app.waitText(".folder .folder-name", /Postausgang/, 20000);
  await app.shot("offline-outbox");
  // Nothing reached the server.
  const state = await server.state();
  assert.ok(state.mailboxes.INBOX.some((m) => m.subject === "Alte Rechnung"), "still in the inbox on the server");
  assert.equal((await server.sent()).length, 0);
});

it("lists what waits in the outbox", async () => {
  await clickText(app, ".folder .folder-name", "Postausgang");
  await app.waitText(".list-pane, .outbox", /Nachricht senden/, 10000);
  await app.shot("offline-outbox-list");
});

it("replays the queue when the server is back", async () => {
  await server.online();
  await app.click('.account-status [aria-label="Jetzt erneut versuchen"]').catch(() => {});
  await waitOnServer(app, server, "Archive", "Alte Rechnung", { timeout: 40000 });
  await waitOnServer(app, server, "INBOX", "Alte Rechnung", { present: false });
  await waitOnServer(app, server, "INBOX", "Frage zum Urlaub", { where: (m) => m.flagged && m.answered });
  await app.waitUntil(async () => (await server.sent()).length === 1, 20000, "the answer went out");
  assert.match(await server.sentRaw(0), /Am Samstag fr(ü|=C3=BC)h\./);
  await waitOnServer(app, server, "Sent", "Re: Frage zum Urlaub");
  // The outbox is empty again and goes away.
  await app.waitUntil(
    async () => !(await app.browser.execute(() => [...document.querySelectorAll(".folder .folder-name")].some((e) => e.textContent === "Postausgang"))),
    20000,
    "outbox gone",
  );
  await openFolder(app, "maria@example.org", "Posteingang");
  assert.deepEqual(await subjects(app), ["Frage zum Urlaub", "Wochenplan"]);
});
