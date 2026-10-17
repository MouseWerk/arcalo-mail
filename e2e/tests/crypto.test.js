// Signed and encrypted mail with OpenPGP against Jonas, the test server's other side (its own
// key, `/pgp/…`): a key made in the settings, Jonas's key imported, a message signed and
// encrypted in the compose sheet that Jonas opens and whose signature he checks, and Jonas's
// encrypted answer with its lock and check, remote images kept off.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, menuItem, openFolder, openMessage, openSettings, waitOnServer } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const control = (p, init) => fetch(`http://127.0.0.1:${server.control}${p}`, init);

before(async () => {
  server = await startServer();
  app = await launch();
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("makes a key in the settings and imports Jonas's", async () => {
  await openSettings(app, "Verschlüsselung");
  await clickText(app, ".set-section .btn span", "OpenPGP-Schlüssel für maria@example.org erstellen");
  await app.waitText(".set-section .rule-name", /maria@example\.org/, 20000);
  const armored = await (await control("/pgp/public")).text();
  await clickText(app, ".set-section .btn span", "OpenPGP-Schlüssel importieren");
  await app.fill(".dialog textarea", armored);
  await clickText(app, ".dialog .btn-primary", "Importieren");
  await app.waitText(".set-section .rule-name", /jonas@example\.org/, 10000);
  const keys = await app.invoke("crypto_keys");
  assert.deepEqual(keys.map((k) => [k.email, k.own]).sort(), [["jonas@example.org", false], ["maria@example.org", true]]);
  await app.shot("crypto-settings");
  await app.keys("Escape");
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
});

it("signs and encrypts a message that Jonas opens", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("jonas@example.org");
  await app.keys("Enter");
  await app.fill("#compose-subject", "Vertrag");
  await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor, .compose .compose-body");
    ed.focus();
  });
  await app.type("Der Vertrag liegt im Tresor.");
  await app.waitFor('.compose-foot [data-compose="protect"]', 8000);
  await app.click('.compose-foot [data-compose="protect"]');
  // The first „Verschlüsseln“ is OpenPGP's (S/MIME has no certificate here).
  await menuItem(app, "Verschlüsseln");
  await app.waitText('.compose-foot [data-compose="protect"]', /Verschlüsselt \(OpenPGP\)/);
  await app.shot("compose-encrypted");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await server.sent()).length === 1, 25000, "sent over SMTP");
  const raw = await server.sentRaw(0);
  assert.match(raw, /multipart\/encrypted/);
  assert.doesNotMatch(raw, /Tresor/, "the text does not travel in the clear");
  assert.match(raw, /^Autocrypt: addr=maria@example\.org/m);
  const read = await (await control("/pgp/sent?i=0")).json();
  assert.equal(read.decrypted, true, JSON.stringify(read));
  assert.equal(read.signature, "valid");
  assert.match(read.text, /Der Vertrag liegt im Tresor/);
});

it("opens Jonas's encrypted answer with its lock and check", async () => {
  const r = await control(`/pgp/deliver?subject=${encodeURIComponent("Re: Vertrag")}`, { method: "POST", body: 'Danke, ich hole ihn ab. <img src="https://tracker.example/x.png">' });
  assert.equal(r.status, 200, await r.text());
  await openMessage(app, "Re: Vertrag", 30000);
  await app.waitText(".protect-badge", /Verschlüsselt \(OpenPGP\)/, 10000);
  await app.waitText(".protect-badge", /Signiert von/);
  await app.waitText(".protect-note", /Externe Bilder bleiben/);
  await app.browser.switchFrame(await app.$(".mail-frame"));
  assert.match(await app.browser.execute(() => document.body.innerText), /ich hole ihn ab/);
  await app.browser.switchFrame(null);
  await app.shot("read-encrypted");
});

it("imports an own key protected with a passphrase and signs with it", async () => {
  const armored = await (await control("/pgp/protected?email=maria@example.org")).text();
  assert.match(armored, /BEGIN PGP PRIVATE KEY BLOCK/);
  await openSettings(app, "Verschlüsselung");
  await clickText(app, ".set-section .btn span", "OpenPGP-Schlüssel importieren");
  await app.fill(".dialog textarea", armored);
  await clickText(app, ".dialog .btn-primary", "Importieren");
  // The dialog asks for the passphrase and checks it.
  await app.waitFor('.dialog input[type="password"]', 10000);
  await app.fill('.dialog input[type="password"]', "falsch");
  await clickText(app, ".dialog .btn-primary", "Importieren");
  await app.waitText(".dialog .field-error", /Passphrase stimmt nicht/, 10000);
  await app.fill('.dialog input[type="password"]', "geheim");
  await clickText(app, ".dialog .btn-primary", "Importieren");
  await app.waitUntil(async () => (await app.$$(".dialog")).length === 0, 10000, "imported");
  const own = (await app.invoke("crypto_keys")).filter((k) => k.own && k.email === "maria@example.org");
  assert.equal(own.length, 2, JSON.stringify(own));
  await app.keys("Escape");
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
  // Given a moment ago, the passphrase is still in memory: a signed message goes out with the new key.
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("jonas@example.org");
  await app.keys("Enter");
  await app.fill("#compose-subject", "Signiert");
  await app.browser.execute(() => document.querySelector(".compose .editor, .compose .compose-body").focus());
  await app.type("Mit dem neuen Schlüssel.");
  // Jonas's answer carried his key with prefer-encrypt=mutual (Autocrypt): the sheet starts encrypted.
  await app.waitText('.compose-foot [data-compose="protect"]', /Verschlüsselt \(OpenPGP\)/, 8000);
  await app.click('.compose-foot [data-compose="protect"]');
  await menuItem(app, "Signieren");
  await new Promise((r) => setTimeout(r, 300));
  const before = (await server.sent()).length;
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await server.sent()).length === before + 1, 25000, "sent over SMTP");
  const read = await (await control(`/pgp/sent?i=${before}`)).json();
  assert.equal(read.decrypted, true, JSON.stringify(read));
  assert.equal(read.signature, "valid", JSON.stringify(read));
  assert.match(read.text, /Mit dem neuen Schlüssel/);
});

it("keeps the draft of an encrypted message encrypted on the server and opens it again", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("jonas@example.org");
  await app.keys("Enter");
  await app.fill("#compose-subject", "Geheimer Entwurf");
  await app.browser.execute(() => document.querySelector(".compose .editor, .compose .compose-body").focus());
  await app.type("Nur für mich lesbar.");
  // Jonas's key came with prefer-encrypt=mutual: the sheet starts encrypted.
  await app.waitText('.compose-foot [data-compose="protect"]', /Verschlüsselt \(OpenPGP\)/, 8000);
  await app.keys(["Control", "s"]);
  await app.waitText(".toast-title", /Entwurf gespeichert/);
  await waitOnServer(app, server, "Drafts", "Geheimer Entwurf", { where: (m) => m.draft, timeout: 25000 });
  const uid = (await server.state()).mailboxes.Drafts.find((m) => m.subject === "Geheimer Entwurf" && !m.deleted).uid;
  const raw = await (await control(`/raw?mailbox=Drafts&uid=${uid}`)).text();
  assert.match(raw, /multipart\/encrypted/);
  assert.match(raw, /^X-Arcalo-Mail-Protect: pgp; sign=\d; encrypt=1/m);
  assert.doesNotMatch(raw, /Nur für mich/, "the draft's text is not readable on the server");
  // Closed and opened again from Drafts: decrypted, still to be encrypted.
  await app.click(`.compose [aria-label="Schließen und Entwurf behalten"]`);
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
  await openFolder(app, "maria@example.org", "Entwürfe");
  await openMessage(app, "Geheimer Entwurf", 30000);
  await clickText(app, ".notice .btn", "Weiterschreiben");
  await app.waitFor(".compose");
  await app.waitText(".compose .editor, .compose .compose-body", /Nur für mich lesbar/, 10000);
  await app.waitText('.compose-foot [data-compose="protect"]', /Verschlüsselt \(OpenPGP\)/, 8000);
  await app.shot("compose-encrypted-draft");
  await app.click(`.compose [aria-label="Schließen und Entwurf behalten"]`);
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
});
