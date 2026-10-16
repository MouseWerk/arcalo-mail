// Writing: a new message sent to the test server's SMTP and filed in Sent, a reply with its
// headers and the \Answered flag, an attachment dropped on the sheet, undo send, and a draft
// saved to Drafts.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, waitOnServer } from "../lib/mail.js";

let app;
let server;
let tmp;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

before(async () => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-files-"));
  server = await startServer();
  await server.deliver(message({ subject: "Treffen am Montag", id: "treffen@example.org", body: "Passt dir Montag um neun?" }), { date: Math.floor(Date.now() / 1000) - 60 });
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
  fs.rmSync(tmp, { recursive: true, force: true });
});

async function fillTo(address) {
  await app.click(".compose .address-field .chip-input");
  await app.type(address);
  await app.keys("Enter");
}

async function typeBody(text) {
  await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor, .compose .compose-body");
    ed.focus();
    const r = document.createRange();
    r.setStart(ed.firstChild ?? ed, 0);
    r.collapse(true);
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  });
  await app.type(text);
}

async function newMessage() {
  // Keys go to the window, not to a field.
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
}

async function sentCount() {
  return (await server.sent()).length;
}

it("sends a new message and files it in Sent", async () => {
  await newMessage();
  await fillTo("lea@example.org");
  await app.fill("#compose-subject", "Unterlagen für Freitag");
  await typeBody("Hallo Lea, die Unterlagen kommen morgen.");
  await app.shot("compose-new");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitText(".toast-title", /Wird gesendet/);
  await app.waitUntil(async () => (await sentCount()) === 1, 20000, "sent over SMTP");
  const sent = (await server.sent())[0];
  assert.equal(sent.subject, "Unterlagen für Freitag");
  assert.deepEqual(sent.to, ["lea@example.org"]);
  const raw = await server.sentRaw(0);
  assert.match(raw, /die Unterlagen kommen morgen/);
  assert.match(raw, /^From: "?Maria Muster"? <maria@example\.org>/m);
  await waitOnServer(app, server, "Sent", "Unterlagen für Freitag", { where: (m) => m.seen });
});

it("replies with the right headers and marks the original answered", async () => {
  await openMessage(app, "Treffen am Montag");
  await app.keys("r");
  await app.waitFor(".compose");
  assert.equal(await (await app.$("#compose-subject")).getValue(), "Re: Treffen am Montag");
  await typeBody("Montag passt.");
  await app.keys(["Control", "Enter"]);
  await app.waitUntil(async () => (await sentCount()) === 2, 20000, "reply sent");
  const raw = await server.sentRaw(1);
  assert.match(raw, /^In-Reply-To: <treffen@example\.org>/m);
  assert.match(raw, /^To: "?Jonas Berger"? <jonas@example\.org>/m);
  assert.match(raw, /Montag passt\./);
  assert.match(raw, /> Passt dir Montag um neun\?/, "the quote goes along");
  await waitOnServer(app, server, "INBOX", "Treffen am Montag", { where: (m) => m.answered });
});

it("takes a dropped file as an attachment", async () => {
  const file = path.join(tmp, "plan.txt");
  fs.writeFileSync(file, "Punkt 1\nPunkt 2\n");
  await newMessage();
  await fillTo("lea@example.org");
  await app.fill("#compose-subject", "Plan");
  await app.browser.executeAsync((p, done) => {
    window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "tauri://drag-drop", payload: { paths: [p], position: { x: 10, y: 10 } } }).then(done, done);
  }, file);
  await app.waitText(".compose-attachments .chip-text", /plan\.txt/);
  await app.shot("compose-attachment");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await sentCount()) === 3, 20000, "sent with the file");
  const raw = await server.sentRaw(2);
  assert.match(raw, /filename="?plan\.txt"?/);
  assert.match(raw, /UHVua3QgMQpQdW5rdCAyCg==|Punkt 1/);
});

it("undoes a send within the undo time", async () => {
  await newMessage();
  await fillTo("lea@example.org");
  await app.fill("#compose-subject", "Doch nicht");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await clickText(app, ".toast-action", "Rückgängig");
  await app.waitText(".toast-title", /Senden abgebrochen/);
  // The message is back in the sheet.
  await app.waitFor(".compose");
  assert.equal(await (await app.$("#compose-subject")).getValue(), "Doch nicht");
  await sleep(7000);
  assert.equal(await sentCount(), 3, "nothing went out");
});

it("saves a draft to Drafts", async () => {
  await app.fill("#compose-subject", "Entwurf für später");
  await app.keys(["Control", "s"]);
  await app.waitText(".toast-title", /Entwurf gespeichert/);
  await waitOnServer(app, server, "Drafts", "Entwurf für später", { where: (m) => m.draft });
  // Saving again replaces the copy on the server.
  await app.fill("#compose-subject", "Entwurf, zweite Fassung");
  await app.keys(["Control", "s"]);
  await waitOnServer(app, server, "Drafts", "Entwurf, zweite Fassung");
  await waitOnServer(app, server, "Drafts", "Entwurf für später", { present: false });
});
