// Attachments that could run code: a program whose name hides its extension behind a
// right-to-left override is shown with its real name and never opened, only saved; an Office
// document with macros opens only after a warning.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

before(async () => {
  server = await startServer();
  await server.deliver(
    message({
      from: "Lea Wagner <lea@example.org>",
      subject: "Unterlagen zum Projekt",
      body: "Anbei die Rechnung.",
      attachments: [
        { name: "Rechnung_2026‮fdp.exe", type: "application/octet-stream", data: "MZ not really a program" },
        { name: "bericht.docm", type: "application/vnd.ms-word.document.macroEnabled.12", data: "PK not really a document" },
      ],
    }),
  );
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

const names = async () => {
  const out = [];
  for (const n of await app.$$(".reading-pane .attachment-name")) out.push(await app.textOf(n));
  return out;
};

it("shows the real extension and never opens a program", async () => {
  await openMessage(app, "Unterlagen zum Projekt");
  await app.waitUntil(async () => (await names()).length === 2, 15000, "two attachments");
  assert.deepEqual(await names(), ["Rechnung_2026fdp.exe", "bericht.docm"], "no right-to-left override in the name");
  await clickText(app, ".reading-pane .attachment-name", "Rechnung_2026fdp.exe");
  await app.waitText(".dialog-title", /Diese Datei öffnet Arcalo Mail nicht/);
  assert.match(await app.text(".dialog-text"), /Programm, ein Skript oder eine Verknüpfung/);
  const buttons = [];
  for (const b of await app.$$(".dialog-foot .btn")) buttons.push(await app.textOf(b));
  assert.deepEqual(buttons, ["Abbrechen", "Speichern"], "saving is the only way");
  await app.shot("attachment-blocked");
  await clickText(app, ".dialog-foot .btn", "Abbrechen");
  await new Promise((r) => setTimeout(r, 500));
  assert.deepEqual(app.opened().filter((l) => l.includes(".exe")), [], "nothing went to a program");
  // The backend refuses too, whatever the UI asks.
  const page = await app.invoke("hub_list", { filter: { kind: null, sender: null, since: null, until: null, query: "", account_id: null }, offset: 0, limit: 50 });
  const exe = page.items.find((i) => i.filename === "Rechnung_2026fdp.exe");
  assert.ok(exe, JSON.stringify(page.items));
  assert.deepEqual(await app.invoke("open_attachment", { id: exe.id, confirmed: true }), { opened: false, risk: "blocked" });
  assert.deepEqual(app.opened().filter((l) => l.includes(".exe")), []);
});

it("opens a document with macros only after the warning", async () => {
  await clickText(app, ".reading-pane .attachment-name", "bericht.docm");
  await app.waitText(".dialog-title", /Dokument mit Makros/);
  assert.equal(app.opened().filter((l) => l.endsWith("bericht.docm")).length, 0, "not before the answer");
  await clickText(app, ".dialog-foot .btn", "Trotzdem öffnen");
  await app.waitUntil(async () => app.opened().some((l) => l.endsWith("bericht.docm")), 10000, "opened after the warning");
});
