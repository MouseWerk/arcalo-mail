// Rules: one in the app (made in the editor, previewed, run on the inbox, then applied to new
// mail) and one on the server (written as a Sieve script over ManageSieve to the test server,
// switched off and deleted there too).

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { choose, clickText, openSettings, waitOnServer } from "../lib/mail.js";

let app;
let server;
let accountId;
const it = guarded(test, () => app);

const sieve = async () => (await fetch(`http://127.0.0.1:${server.control}/sieve`)).json();
const activeScript = async () => {
  const s = await sieve();
  return s.active ? (s.scripts[s.active] ?? "") : "";
};

before(async () => {
  server = await startServer();
  await server.folder("Rechnungen");
  await server.deliver(message({ from: "Shop <billing@shop.example>", subject: "Rechnung 1001", body: "Betrag: 12 Euro" }));
  await server.deliver(message({ from: "Shop <billing@shop.example>", subject: "Rechnung 1002", body: "Betrag: 30 Euro" }));
  await server.deliver(message({ from: "Lea <lea@example.org>", subject: "Kino?", body: "Heute Abend?" }));
  app = await launch();
  accountId = (await addAccount(app, server)).id;
  await app.browser.refresh();
  await app.waitText(".row-subject", /Kino\?/, 20000);
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("makes a rule in the app, previews it and runs it on the inbox", async () => {
  await openSettings(app, "Regeln");
  await clickText(app, ".set-actions .btn span", "Regel hinzufügen");
  await app.waitFor(".rule-editor");
  await app.fill(".rule-editor .rule-grid .input", "Rechnungen ablegen");
  const cond = (await app.$$(".rule-editor .rule-block"))[0];
  await app.browser.execute((e, v) => {
    const i = e.querySelector(".rule-line .input");
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(i, v);
    i.dispatchEvent(new Event("input", { bubbles: true }));
  }, cond, "billing@shop.example");
  const act = (await app.$$(".rule-editor .rule-block"))[1];
  await app.waitUntil(async () => (await act.$$('select[aria-label="Ordner"] option')).length > 2, 10000, "the folders");
  await choose(app, await act.$('select[aria-label="Ordner"]'), "Rechnungen");
  await clickText(app, ".rule-tools .btn span", "Welche Mails passen?");
  await app.waitText(".rule-preview p", /2 Nachrichten auf diesem Gerät passen/);
  assert.match(await app.text(".rule-preview"), /Rechnung 1001[\s\S]*|Rechnung 1002/);
  await app.shot("rules-editor");
  await clickText(app, ".rule-tools .btn span", "Jetzt anwenden");
  await app.waitText(".toast-title", /Die Regel hat 2 Nachrichten erfasst/);
  await waitOnServer(app, server, "Rechnungen", "Rechnung 1001");
  await waitOnServer(app, server, "Rechnungen", "Rechnung 1002");
  await clickText(app, ".dialog-foot .btn-primary span", "Regel speichern");
  await app.waitText(".rule-item .rule-name", /Rechnungen ablegen/);
  assert.match(await app.text(".rule-item .rule-summary"), /Rechnungen/);
});

it("applies the rule to new mail", async () => {
  await server.deliver(message({ from: "Shop <billing@shop.example>", subject: "Rechnung 1003", body: "Betrag: 7 Euro" }));
  await app.invoke("sync_now", { accountId });
  await waitOnServer(app, server, "Rechnungen", "Rechnung 1003", { timeout: 30000 });
  await waitOnServer(app, server, "INBOX", "Rechnung 1003", { present: false });
});

it("writes a server rule as Sieve over ManageSieve", async () => {
  await clickText(app, ".set-actions .btn span", "Regel hinzufügen");
  await app.waitFor(".rule-editor");
  await app.fill(".rule-editor .rule-grid .input", "Newsletter markieren");
  await clickText(app, '.rule-where .segmented [role="radio"]', "Auf dem Server");
  // Nothing answers on the mail server's port 4190: the editor asks where ManageSieve is.
  await app.waitFor(".sieve-server", 15000);
  await app.fill(".sieve-server .input", `127.0.0.1:${server.sieve}`);
  await choose(app, ".sieve-server .select", "plain");
  await clickText(app, ".sieve-server .btn", "Prüfen");
  await app.waitText(".rule-where-note", new RegExp(`Sieve auf 127\\.0\\.0\\.1:${server.sieve}`), 10000);
  const cond = (await app.$$(".rule-editor .rule-block"))[0];
  await choose(app, await cond.$('select[aria-label="Feld"]'), "subject");
  await app.browser.execute((e, v) => {
    const i = e.querySelector(".rule-line .input");
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(i, v);
    i.dispatchEvent(new Event("input", { bubbles: true }));
  }, cond, "Newsletter");
  const act = (await app.$$(".rule-editor .rule-block"))[1];
  await choose(app, await act.$('select[aria-label="Aktion"]'), "mark_read");
  await clickText(app, ".dialog-foot .btn-primary span", "Regel speichern");
  await app.waitText(".rule-item .rule-name", /Newsletter markieren/);
  await app.waitUntil(async () => /Newsletter/.test(await activeScript()), 10000, "the script on the server");
  const script = await activeScript();
  assert.match(script, /require .*"imap4flags"/);
  assert.match(script, /header :contains "subject" "Newsletter"/i);
  assert.match(script, /addflag "\\\\Seen"/);
  await app.shot("rules-list");
});

it("switches a server rule off and deletes it there", async () => {
  await app.click('[aria-label="Newsletter markieren ein oder aus"]');
  await app.waitUntil(async () => !/Newsletter/.test(await activeScript()), 10000, "off on the server");
  await app.click('[aria-label="Newsletter markieren löschen"]');
  await clickText(app, ".dialog-foot .btn", "Löschen");
  await app.waitUntil(async () => (await app.$$(".rule-item")).length === 1, 5000, "one rule left");
  assert.doesNotMatch(await activeScript(), /Newsletter/);
});
