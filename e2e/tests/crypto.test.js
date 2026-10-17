// Signed and encrypted mail with OpenPGP against Jonas, the test server's other side (its own
// key, `/pgp/…`): a key made in the settings, Jonas's key imported, a message signed and
// encrypted in the compose sheet that Jonas opens and whose signature he checks, and Jonas's
// encrypted answer with its lock and check, remote images kept off.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, menuItem, openMessage, openSettings } from "../lib/mail.js";

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
