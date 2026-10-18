// Folders and several messages at once: a folder created, renamed and deleted from the sidebar
// reaches the server; messages picked with Ctrl+click and Shift+click are archived, marked read
// and deleted together.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, subjects, waitOnServer, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

const SUBJECTS = ["Eins", "Zwei", "Drei", "Vier", "Fünf"];

before(async () => {
  server = await startServer();
  const now = Math.floor(Date.now() / 1000);
  for (const [i, subject] of SUBJECTS.entries()) {
    await server.deliver(message({ subject, body: `Nachricht ${subject}` }), { date: now - 600 + i * 60 });
  }
  app = await launch();
  await addAccount(app, server);
  await app.reload();
  await waitRow(app, "Fünf");
});

after(async () => {
  await app?.close();
  server?.stop();
});

/** The row with this subject, for a click with modifier keys. */
async function row(subject) {
  for (const r of await app.$$(".row")) if ((await app.textOf(r)).includes(subject)) return r;
  throw new Error(`no row ${subject}`);
}

/** Clicks a row with Ctrl or Shift held (the click event the list reads, with its keys). */
async function clickWith(key, subject) {
  const r = await row(subject);
  await app.browser.execute(
    (el, k) => el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: k === "Control", shiftKey: k === "Shift" })),
    r,
    key,
  );
}

/** Points at an element, so the buttons that show on hover appear. */
async function hover(sel) {
  await (await app.$(sel)).moveTo();
}

it("creates, renames and deletes a folder", async () => {
  await hover(".account-head");
  await app.click('.account [aria-label="Neuer Ordner"]');
  await app.waitFor(".dialog input");
  await app.fill(".dialog input", "Projekte");
  await clickText(app, ".dialog-foot .btn", "Anlegen");
  await app.waitText(".folder .folder-name", /^Projekte$/, 10000);
  await app.waitUntil(async () => "Projekte" in (await server.state()).mailboxes, 20000, "folder on the server");

  await hover('.folder-item:has([aria-label="Aktionen für „Projekte“"])');
  await app.click('[aria-label="Aktionen für „Projekte“"]');
  await clickText(app, ".menu-item", "Umbenennen");
  await app.fill(".dialog input", "Kunden");
  await clickText(app, ".dialog-foot .btn", "Umbenennen");
  await app.waitUntil(async () => {
    const boxes = (await server.state()).mailboxes;
    return "Kunden" in boxes && !("Projekte" in boxes);
  }, 20000, "renamed on the server");
  await app.waitText(".folder .folder-name", /^Kunden$/, 10000);
  await app.shot("folder-renamed");

  await hover('.folder-item:has([aria-label="Aktionen für „Kunden“"])');
  await app.click('[aria-label="Aktionen für „Kunden“"]');
  await clickText(app, ".menu-item", "Ordner löschen");
  await clickText(app, ".dialog-foot .btn", "Ordner löschen");
  await app.waitUntil(async () => !("Kunden" in (await server.state()).mailboxes), 20000, "deleted on the server");
  await app.waitUntil(
    async () => !(await app.browser.execute(() => [...document.querySelectorAll(".folder .folder-name")].some((e) => e.textContent === "Kunden"))),
    10000,
    "gone from the sidebar",
  );
});

it("archives several messages at once", async () => {
  await (await row("Fünf")).click();
  await clickWith("Control", "Drei");
  await app.waitText(".bulk-count", /2 ausgewählt/);
  await app.shot("bulk-two");
  await app.click('.bulk-bar [aria-label="Archivieren"]');
  await waitRow(app, "Fünf", false);
  await waitRow(app, "Drei", false);
  await waitOnServer(app, server, "Archive", "Fünf", { timeout: 30000 });
  await waitOnServer(app, server, "Archive", "Drei");
  await waitOnServer(app, server, "INBOX", "Drei", { present: false });
});

it("marks a range read and deletes it", async () => {
  assert.deepEqual(await subjects(app), ["Vier", "Zwei", "Eins"]);
  await (await row("Vier")).click();
  await clickWith("Shift", "Eins");
  await app.waitText(".bulk-count", /3 ausgewählt/);
  await app.click('.bulk-bar [aria-label="Als gelesen markieren"]');
  await waitOnServer(app, server, "INBOX", "Zwei", { where: (m) => m.seen });
  await waitOnServer(app, server, "INBOX", "Eins", { where: (m) => m.seen });
  await (await row("Vier")).click();
  await clickWith("Shift", "Eins");
  await app.waitText(".bulk-count", /3 ausgewählt/);
  await app.click('.bulk-bar [aria-label="Löschen"]');
  await app.waitUntil(async () => (await subjects(app)).length === 0, 10000, "list empty");
  for (const s of ["Vier", "Zwei", "Eins"]) await waitOnServer(app, server, "Trash", s, { timeout: 30000 });
  assert.equal((await server.state()).mailboxes.INBOX.length, 0);
});
