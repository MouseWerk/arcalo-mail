// The Screener: off by default; switched on, first-time senders wait in „Neue Absender“, while
// contacts, replies and mail from before it was on come through. „Zulassen“ lets a sender in,
// „Abweisen“ moves its mail to „Screened Out“ on the server, now and when more arrives;
// nothing is deleted.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openView, subjects, viewCount, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const now = () => Math.floor(Date.now() / 1000);

before(async () => {
  server = await startServer();
  // Mail from before the Screener, and a message the user wrote to Lea (she is a contact).
  await server.deliver(message({ subject: "Von früher", body: "Schon da." }), { date: now() - 600 });
  await server.deliver(message({ from: "Maria Muster <maria@example.org>", to: "Lea <lea@example.org>", subject: "Frage an Lea", id: "frage@example.org", body: "?" }), {
    mailbox: "Sent",
    seen: true,
    date: now() - 500,
  });
  app = await launch();
  await addAccount(app, server);
  await app.reload();
  await waitRow(app, "Von früher");
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("is off until switched on, then holds back first-time senders", async () => {
  const settings = await app.invoke("get_settings");
  assert.equal(settings.screener, false, "off by default");
  assert.equal(await viewCount(app, "Neue Absender"), null);
  await app.invoke("set_settings", { settings: { ...settings, screener: true } });
  const after = await app.invoke("get_settings");
  assert.ok(after.screener_since > 0, "the start is noted");
  const later = after.screener_since + 2;
  await server.deliver(message({ from: "Neu <neu@example.net>", subject: "Hallo von neu", body: "Hallo!" }), { date: later });
  await server.deliver(message({ from: "Werbung <angebote@werbung.example>", subject: "Großes Angebot", body: "Kauf!" }), { date: later });
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Re: Frage an Lea", inReplyTo: "frage@example.org", body: "Antwort." }), { date: later });
  await server.deliver(message({ from: "Fremd <fremd@example.net>", subject: "Re: Frage an Lea", inReplyTo: "frage@example.org", body: "Auch eine Antwort." }), { date: later + 1 });
  await app.reload();
  await waitRow(app, "Re: Frage an Lea", true, 30000);
  await app.waitUntil(async () => (await viewCount(app, "Neue Absender")) === 2, 20000, "two senders wait");
  const list = await subjects(app);
  assert.ok(list.includes("Von früher"), "mail from before stays");
  assert.ok(!list.includes("Hallo von neu") && !list.includes("Großes Angebot"), list.join(", "));
});

it("lets a sender in", async () => {
  await openView(app, "Neue Absender");
  await app.waitText(".screener-item", /Hallo von neu/);
  await app.shot("screener");
  for (const item of await app.$$(".screener-item")) {
    if ((await app.textOf(item)).includes("Hallo von neu")) {
      await (await item.$(".btn:not(.btn-quiet)")).click();
      break;
    }
  }
  await app.waitText(".toast-title", /Neu kommt jetzt durch/);
  await app.waitUntil(async () => (await app.$$(".screener-item")).length === 1, 10000, "one left");
  await clickText(app, ".account .folder-name", "Posteingang");
  await waitRow(app, "Hallo von neu");
});

it("turns a sender away: its mail moves to Screened Out, now and later", async () => {
  await openView(app, "Neue Absender");
  await app.waitText(".screener-item", /Großes Angebot/);
  await (await app.$(".screener-item .btn-quiet")).click();
  await app.waitText(".toast-title", /Werbung abgewiesen/);
  await waitOnServer(app, server, "Screened Out", "Großes Angebot");
  await waitOnServer(app, server, "INBOX", "Großes Angebot", { present: false });
  await app.waitText(".smart-empty", /Keine neuen Absender warten/);
  await server.deliver(message({ from: "Werbung <angebote@werbung.example>", subject: "Noch ein Angebot", body: "Kauf!" }));
  await waitOnServer(app, server, "Screened Out", "Noch ein Angebot", { timeout: 30000 });
  await waitOnServer(app, server, "INBOX", "Noch ein Angebot", { present: false });
  // The folder shows by its name, and the settings can let the sender in again.
  await app.waitText(".account .folder-name", /Abgewiesen/);
  const blocked = await app.invoke("blocked_senders");
  assert.deepEqual(blocked, ["angebote@werbung.example"]);
});
