// Contacts: a CardDAV address book connected in the settings (the test server's `/dav`), its
// names in the address field, its picture next to the sender, and the contact card with the
// recent mail, the files and „Neue Mail“. Pictures never come from inside a message.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { choose, clickText, openMessage, openSettings } from "../lib/mail.js";
import { landscape, pdf } from "../lib/png.js";

let app;
let server;
const it = guarded(test, () => app);

const VCARD = [
  "BEGIN:VCARD",
  "VERSION:3.0",
  "FN:Lea Wagner",
  "N:Wagner;Lea;;;",
  "EMAIL;TYPE=INTERNET:lea@example.org",
  "TEL;TYPE=CELL:+49 170 1234567",
  "ORG:Wagner Design",
  `PHOTO;ENCODING=b;TYPE=PNG:${landscape(96, 96, [[214, 170, 140], [236, 206, 180], [120, 92, 80], [96, 120, 150]]).toString("base64")}`,
  "END:VCARD",
  "",
].join("\r\n");

before(async () => {
  server = await startServer();
  await server.deliver(
    message({
      from: "Lea Wagner <lea@example.org>",
      subject: "Entwurf Logo",
      body: "Hier der Entwurf.",
      attachments: [{ name: "logo-entwurf.pdf", type: "application/pdf", data: pdf("Logo") }],
      html: `<p>Hier der Entwurf.</p><img src="${server.webUrl("/img/lea.png")}" alt="">`,
    }),
  );
  await server.deliver(message({ from: "Lea Wagner <lea@example.org>", subject: "Termin Freitag", body: "Passt 10 Uhr?" }));
  await fetch(`http://127.0.0.1:${server.control}/dav-card?name=lea`, { method: "POST", body: VCARD });
  app = await launch();
  await addAccount(app, server);
  await app.reload();
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("connects a CardDAV address book", async () => {
  await openSettings(app, "Kontakte");
  await clickText(app, ".set-actions .btn span", "Adressbuch verbinden");
  await app.waitFor(".dialog .select");
  const dialog = (await app.$$(".dialog")).at(-1);
  await choose(app, await dialog.$(".select"), "5");
  const inputs = await dialog.$$(".input");
  const set = (el, v) =>
    app.browser.execute(
      (e, value) => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(e, value);
        e.dispatchEvent(new Event("input", { bubbles: true }));
      },
      el,
      v,
    );
  await set(inputs[0], `http://127.0.0.1:${server.control}/dav`);
  await set(inputs[1], server.user);
  await set(inputs[2], server.password);
  await clickText(app, ".dialog-foot .btn-primary span", "Verbinden");
  await app.waitText(".rule-item .rule-summary", /1 Kontakt · abgeglichen/, 15000);
  await app.shot("contacts-settings");
  await app.click('.settings-head [aria-label="Schließen"]');
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
});

it("suggests the address book's names while writing", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("Wagn");
  await app.waitText(".suggestions .suggestion", /Lea Wagner[\s\S]*lea@example\.org/);
  await app.keys("Enter");
  await app.waitText(".compose .address-field .chip", /Lea Wagner/);
  await app.click('.compose-foot [aria-label="Verwerfen"]');
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 5000, "compose closed");
});

it("shows the contact's picture and card", async () => {
  await openMessage(app, "Entwurf Logo");
  await app.waitFor(".message-card .avatar-picture img", 10000);
  await app.click(".message-card .avatar-btn");
  await app.waitFor(".contact-card");
  await app.waitText(".contact-card .contact-name", /Lea Wagner/);
  const card = await app.text(".contact-card");
  assert.match(card, /Wagner Design/);
  assert.match(card, /\+49 170 1234567/);
  assert.match(card, /Aus dem Adressbuch/);
  await app.waitText(".contact-card .contact-row-subject", /Termin Freitag/);
  await app.waitText(".contact-card .contact-file .contact-row-subject", /logo-entwurf\.pdf/);
  assert.match(await app.text(".contact-card .contact-counts"), /Erhalten: 2 · Gesendet: 0/);
  await app.shot("contact-card");
  // The picture came from the address book: nothing was fetched from inside the message.
  assert.deepEqual((await server.web()).filter((w) => w.path.includes("/img/")), []);
  await clickText(app, ".contact-card .btn-primary span", "Neue Mail");
  await app.waitFor(".compose");
  await app.waitText(".compose .address-field .chip", /Lea Wagner/);
  await app.click('.compose-foot [aria-label="Verwerfen"]');
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 5000, "compose closed");
});

it("opens a message from the contact card", async () => {
  await app.click(".message-card .avatar-btn");
  await app.waitText(".contact-card .contact-row-subject", /Termin Freitag/);
  await clickText(app, ".contact-card .contact-row-subject", "Termin Freitag");
  await app.waitText(".message-subject", /Termin Freitag/);
  assert.equal((await app.$$(".contact-card")).length, 0);
});
