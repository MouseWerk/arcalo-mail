// A POP3 account set up by hand: the setup checks POP3 and SMTP on the test server, the mail
// is downloaded into the folders on this device and stays on the server, new mail comes once
// (UIDL), archiving works locally, and a sent message is filed in the local „Gesendet“.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { message, startServer } from "../lib/server.js";
import { choose, clickText, openFolder, openMessage, subjects, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);

before(async () => {
  server = await startServer();
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Fotos vom Wochenende", body: "Anbei die Fotos." }), { date: now - 300 });
  await server.deliver(message({ from: "Jonas <jonas@example.org>", subject: "Treffen Dienstag", body: "Passt dir 15 Uhr?" }), { date: now - 200 });
  app = await launch();
});

after(async () => {
  await app?.close();
  server?.stop();
});

async function setValue(sel, v) {
  await app.fill(sel, v);
}

it("sets up a POP3 account by hand", async () => {
  await app.waitFor(".setup-welcome");
  await app.fill(".setup-fields input[type=email]", server.user);
  await app.fill(".setup-fields input[type=password]", server.password);
  await clickText(app, ".setup-fields .btn", "Von Hand einrichten");
  await clickText(app, '.setup-fields .segmented [role="radio"]', "POP3 und SMTP");
  await app.waitFor("#setup-pop3-host");
  for (const kind of ["pop3", "smtp"]) {
    await choose(app, `#setup-${kind}-security`, "plain");
    await setValue(`#setup-${kind}-host`, "127.0.0.1");
    await setValue(`#setup-${kind}-port`, String(kind === "pop3" ? server.pop3 : server.smtp));
  }
  await app.shot("pop3-setup");
  await clickText(app, ".setup-fields .btn-primary", "Verbindung prüfen");
  await app.waitFor(".setup-done", 20000);
  await app.click(".setup-done .btn-primary");
  await waitRow(app, "Treffen Dienstag");
  await waitRow(app, "Fotos vom Wochenende");
  const [account] = await app.invoke("list_accounts");
  assert.equal(account.transport.kind, "pop3");
  assert.equal(account.transport.leave_on_server, true);
});

it("keeps the mail on the server and fetches new mail once", async () => {
  const inbox = (await server.state()).mailboxes.INBOX ?? [];
  assert.equal(inbox.filter((m) => !m.deleted).length, 2, "still on the server");
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Noch ein Foto", body: "Das hier fehlte." }));
  const [account] = await app.invoke("list_accounts");
  await app.invoke("sync_now", { accountId: account.id });
  await waitRow(app, "Noch ein Foto");
  await app.invoke("sync_now", { accountId: account.id });
  await new Promise((r) => setTimeout(r, 1500));
  const list = await subjects(app);
  assert.equal(list.filter((s) => s === "Fotos vom Wochenende").length, 1, "no second copy");
  assert.equal(list.length, 3);
});

it("archives on this device only", async () => {
  await openMessage(app, "Fotos vom Wochenende");
  await app.keys("e");
  await waitRow(app, "Fotos vom Wochenende", false);
  await openFolder(app, server.user, "Archiv");
  await waitRow(app, "Fotos vom Wochenende");
  const inbox = (await server.state()).mailboxes.INBOX ?? [];
  assert.ok(inbox.some((m) => m.subject === "Fotos vom Wochenende" && !m.deleted), "the server keeps its copy");
});

it("sends over SMTP and files the message in the local Sent folder", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("lea@example.org");
  await app.keys("Enter");
  await app.fill("#compose-subject", "Danke für die Fotos");
  await app.browser.execute(() => document.querySelector(".compose .editor, .compose .compose-body").focus());
  await app.type("Die sind schön geworden.");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await server.sent()).length === 1, 20000, "sent over SMTP");
  await openFolder(app, server.user, "Gesendet");
  await waitRow(app, "Danke für die Fotos");
});
