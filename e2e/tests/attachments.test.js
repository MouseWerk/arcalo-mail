// „Anhänge“: every attachment of the mail in one list, filtered by kind, sender, time and name,
// with the preview of pictures and text, opening a file in its program and the way to its
// message.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { choose, openView } from "../lib/mail.js";
import { landscape, pdf } from "../lib/png.js";

let app;
let server;
const it = guarded(test, () => app);
const now = Math.floor(Date.now() / 1000);
const DAY = 86400;

before(async () => {
  server = await startServer();
  const deliver = (m, ago) => server.deliver(message({ ...m, date: new Date((now - ago) * 1000) }), { date: now - ago });
  await deliver(
    { from: "Lea Wagner <lea@example.org>", subject: "Fotos vom Ausflug", body: "Zwei Bilder.", attachments: [{ name: "see.png", type: "image/png", data: landscape(480, 320) }] },
    3 * 3600,
  );
  await deliver(
    { from: "Steuerbüro Klein <post@klein.example>", subject: "Unterlagen 2025", body: "Bitte prüfen.", attachments: [{ name: "steuererklaerung-2025.pdf", type: "application/pdf", data: pdf("Steuererklärung 2025") }] },
    2 * DAY,
  );
  await deliver(
    { from: "Jonas Berger <jonas@example.org>", subject: "Kosten Sommerfest", body: "Die Tabelle.", attachments: [{ name: "kosten.csv", type: "text/csv", data: "Posten;Betrag\nGetränke;180\nGrill;95\n" }] },
    40 * DAY,
  );
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
  await app.waitText(".row-subject", /Fotos vom Ausflug/, 20000);
});

after(async () => {
  await app?.close();
  server?.stop();
});

const names = async () => {
  const out = [];
  for (const n of await app.$$(".hub-item .hub-name")) out.push(await app.textOf(n));
  return out;
};

it("lists every attachment, newest first", async () => {
  await openView(app, "Anhänge");
  await app.waitUntil(async () => (await names()).length === 3, 15000, "three files");
  assert.deepEqual(await names(), ["see.png", "steuererklaerung-2025.pdf", "kosten.csv"]);
  assert.match(await app.text(".hub-item:nth-child(2) .hub-meta"), /Steuerbüro Klein/);
  await app.shot("attachments-hub");
});

it("filters by kind, sender, time and name", async () => {
  await choose(app, '.hub-filters [aria-label="Art"]', "images");
  await app.waitUntil(async () => JSON.stringify(await names()) === '["see.png"]', 5000, "pictures only");
  await choose(app, '.hub-filters [aria-label="Art"]', "");
  await choose(app, '.hub-filters [aria-label="Absender"]', "jonas@example.org");
  await app.waitUntil(async () => JSON.stringify(await names()) === '["kosten.csv"]', 5000, "Jonas only");
  await choose(app, '.hub-filters [aria-label="Absender"]', "");
  await choose(app, '.hub-filters [aria-label="Zeitraum"]', "month");
  await app.waitUntil(async () => (await names()).length === 2, 5000, "the last month");
  await choose(app, '.hub-filters [aria-label="Zeitraum"]', "all");
  await app.fill(".hub-search .search-input", "steuer");
  await app.waitUntil(async () => JSON.stringify(await names()) === '["steuererklaerung-2025.pdf"]', 5000, "by name");
  await app.fill(".hub-search .search-input", "");
  await app.waitUntil(async () => (await names()).length === 3, 5000, "all again");
});

it("previews pictures and text", async () => {
  await (await app.$$(".hub-item .hub-main"))[0].click();
  await app.waitFor(".preview img");
  assert.equal(await app.text(".preview figcaption"), "see.png");
  await app.click('.preview [aria-label="Schließen"]');
  await (await app.$$(".hub-item .hub-main"))[2].click();
  await app.waitText(".preview .preview-text", /Getränke;180/);
  await app.shot("attachments-text-preview");
  await app.click('.preview [aria-label="Schließen"]');
  await app.waitUntil(async () => (await app.$$(".preview")).length === 0, 5000, "preview closed");
});

it("opens a file in its program and goes to its message", async () => {
  await (await app.$$(".hub-item .hub-main"))[1].click();
  await app.waitUntil(async () => app.opened().some((l) => l.endsWith("steuererklaerung-2025.pdf")), 10000, "the PDF went to its program");
  await (await app.$$('.hub-item [title="Nachricht öffnen"]'))[1].click();
  await app.waitText(".message-subject", /Unterlagen 2025/);
});
