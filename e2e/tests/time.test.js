// Time tools: a snoozed message waits in „Snoozed“ on the server and comes back unread at the
// top; a message sent later goes out at its time (restart.test.js: at the next start when the
// app was closed then); a reminder comes due when no answer arrived, and stays away when one did.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, localTime, menuItem, openFolder, openMessage, openView, subjects, viewCount, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The clock looks at the time tools every second instead of every ten.
const env = { ARCALO_MAIL_TICK_SECS: "1" };

before(async () => {
  server = await startServer();
  await server.deliver(message({ subject: "Lies mich später", body: "Kein Eilt." }), { date: now - 300, seen: true });
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Frage zum Angebot", id: "angebot@example.org", body: "Wie sieht es aus?" }), { date: now - 200 });
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Termin am Freitag", id: "termin@example.org", body: "Passt Freitag?" }), { date: now - 150 });
  await server.deliver(message({ subject: "Ganz neu", body: "Oben." }), { date: now - 100, seen: true });
  app = await launch({ env });
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

async function pickTime(secondsAhead, confirm) {
  await app.waitFor(".when-dialog");
  await app.fill(".when-dialog .when-input", localTime(secondsAhead));
  await clickText(app, ".when-dialog .dialog-foot .btn-primary", confirm);
}

it("snoozes a message and brings it back unread at the top", async () => {
  await openMessage(app, "Lies mich später");
  await app.click('.reading-pane .toolbar [data-action="snooze"]');
  await app.waitFor(".menu");
  await app.shot("snooze-menu");
  await menuItem(app, "Zeitpunkt wählen…");
  await pickTime(12, "Zurückstellen");
  await app.waitText(".toast-title", /Zurückgestellt bis Heute/);
  await waitRow(app, "Lies mich später", false);
  await waitOnServer(app, server, "Snoozed", "Lies mich später");
  await waitOnServer(app, server, "INBOX", "Lies mich später", { present: false });
  // The counts follow the sync a moment later (its notices are gathered, App.tsx).
  await app.waitUntil(async () => (await viewCount(app, "Zurückgestellt")) === 1, 5000, "the snoozed view with one message");
  await openView(app, "Zurückgestellt");
  await waitRow(app, "Lies mich später");
  assert.match(await app.text(".row-due"), /Heute, \d\d:\d\d/);
  // Due: back in the inbox, unread, on top.
  await clickText(app, ".account .folder-name", "Posteingang");
  await waitRow(app, "Lies mich später", true, 40000);
  await app.waitUntil(async () => (await subjects(app))[0] === "Lies mich später", 20000, "on top");
  await app.waitUntil(async () => {
    const row = (await app.$$(".row"))[0];
    return (await row.getAttribute("data-unread")) === "true";
  }, 10000, "unread");
  await waitOnServer(app, server, "INBOX", "Lies mich später", { where: (m) => !m.seen });
  await waitOnServer(app, server, "Snoozed", "Lies mich später", { present: false });
});

async function compose(to, subject) {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type(to);
  await app.keys("Enter");
  await app.fill("#compose-subject", subject);
}

it("sends a message later, at its time", async () => {
  await compose("lea@example.org", "Später gesendet");
  await app.click(".compose .split-btn-more");
  await menuItem(app, "Zeitpunkt wählen…");
  await app.waitFor(".when-dialog");
  assert.match(await app.text(".when-dialog .when-note"), /beim nächsten Start/);
  await pickTime(10, "Planen");
  await app.waitText(".toast-title", /Geplant für Heute/);
  await app.waitUntil(async () => (await viewCount(app, "Geplant")) === 1, 10000, "listed as scheduled");
  await openView(app, "Geplant");
  await app.waitText(".scheduled-item", /Später gesendet/);
  assert.equal((await server.sent()).length, 0, "not yet");
  await app.waitUntil(async () => (await server.sent()).some((m) => m.subject === "Später gesendet"), 40000, "sent at its time");
  await app.waitText(".smart-empty", /Keine Nachrichten geplant/);
});

it("reminds when no answer came, and not when one did", async () => {
  await clickText(app, ".account .folder-name", "Posteingang");
  for (const subject of ["Frage zum Angebot", "Termin am Freitag"]) {
    await openMessage(app, subject);
    await app.click('.reading-pane .toolbar [data-action="more"]');
    await menuItem(app, "Erinnern, wenn keine Antwort");
    await menuItem(app, "Zeitpunkt wählen…");
    await pickTime(12, "Erinnern");
    await app.waitText(".mark-note", /Erinnerung Heute, \d\d:\d\d, falls keine Antwort kommt/);
  }
  // Lea answers the appointment in time.
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Re: Termin am Freitag", inReplyTo: "termin@example.org", body: "Ja." }));
  await app.waitText(".toast-title", /Noch keine Antwort: Frage zum Angebot/, 40000);
  await app.waitUntil(async () => (await viewCount(app, "Nachfassen")) === 1, 10000, "one reminder");
  await openView(app, "Nachfassen");
  await waitRow(app, "Frage zum Angebot");
  assert.deepEqual(await subjects(app), ["Frage zum Angebot"]);
  await openMessage(app, "Frage zum Angebot");
  await app.waitText(".reminder-banner", /Noch keine Antwort/);
  await clickText(app, ".reminder-banner .btn", "Erledigt");
  await waitRow(app, "Frage zum Angebot", false);
});

it("asks for a reminder when sending", async () => {
  await compose("lea@example.org", "Mit Erinnerung");
  await app.click(".compose .remind-btn");
  await menuItem(app, "Nach 3 Tagen");
  assert.match(await app.text(".compose .remind-btn"), /Erinnerung nach 3 Tagen/);
  await clickText(app, ".compose-foot .btn-primary span", "Senden");
  await app.waitUntil(async () => (await server.sent()).some((m) => m.subject === "Mit Erinnerung"), 20000, "sent");
  await waitOnServer(app, server, "Sent", "Mit Erinnerung");
  await openFolder(app, "maria@example.org", "Gesendet");
  await openMessage(app, "Mit Erinnerung");
  await app.waitText(".mark-note", /Erinnerung .*falls keine Antwort kommt/);
});
