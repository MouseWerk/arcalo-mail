// Settings → Sicherheit against the real app: the app lock (a PIN set in the settings, the lock
// after the idle time, commands refused while locked, wrong and right PINs, locked again at the
// next start), the encryption on the device (the database switched while the app runs, the
// recovery key, a start without the key on the recovery screen, back to unencrypted), and the
// phishing warning with „Das ist in Ordnung“.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, openSettings } from "../lib/mail.js";

let app;
let server;
let dataDir;
let recovery = "";
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const header = () => fs.readFileSync(path.join(dataDir, "mail.db")).subarray(0, 16).toString("latin1");

before(async () => {
  server = await startServer();
  dataDir = fs.mkdtempSync(path.join(fs.realpathSync(process.env.TMPDIR ?? "/tmp"), "arcalo-mail-e2e-security-"));
  app = await launch({ dataDir });
  await addAccount(app, server);
  await server.deliver(message({ subject: "Gehaltsabrechnung Oktober", body: "Vertraulicher Inhalt der Abrechnung." }));
  await app.browser.refresh();
  await app.waitText(".row-subject", /Gehaltsabrechnung Oktober/, 30000);
});

after(async () => {
  await app?.close();
  server?.stop();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

const closeSettings = async () => {
  await app.click('.settings-head [aria-label="Schließen"]');
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
};

it("sets a PIN and locks after the idle time", async () => {
  await openSettings(app, "Sicherheit");
  await clickText(app, "[data-security] [role=radio]", "Bei Inaktivität");
  await app.waitFor("[data-pin-new]");
  await app.fill("[data-pin-new]", "4711");
  await app.fill("[data-pin-repeat]", "4711");
  await app.click("[data-pin-save]");
  await app.waitFor("[data-lock-now]");
  const status = await app.invoke("applock_status");
  assert.equal(status.config.mode, "idle");
  assert.equal(status.has_pin, true);
  // The PIN is a hash in the credential store, never the PIN itself.
  const secrets = fs.readFileSync(path.join(dataDir, "secrets.json"), "utf8");
  assert.match(secrets, /argon2id/);
  assert.doesNotMatch(secrets, /"4711"/);
  await app.shot("security-settings");
  await closeSettings();
  // Ten minutes without input (the debug build's test seam): the app locks.
  await app.invoke("applock_test_idle", { seconds: 600 });
  await app.waitFor("[data-lock-screen]", 10000);
  assert.equal((await app.$$(".row-subject")).length, 0, "nothing of the mail behind the lock screen");
  assert.doesNotMatch(await app.browser.execute(() => document.body.innerText), /Gehaltsabrechnung/);
  await assert.rejects(app.invoke("list_accounts"), /app-locked/);
  await app.shot("lock-screen");
});

it("refuses a wrong PIN and opens with the right one", async () => {
  await app.fill(".lock-pin", "0000");
  await app.keys("Enter");
  await app.waitText(".lock-msg", /Falsche PIN/);
  await app.fill(".lock-pin", "4711");
  await app.keys("Enter");
  await app.waitText(".row-subject", /Gehaltsabrechnung Oktober/, 10000);
  assert.ok((await app.invoke("list_accounts")).length === 1);
});

it("is locked again at the next start", async () => {
  await app.close();
  app = await launch({ dataDir });
  await app.waitFor("[data-lock-screen]", 15000);
  await app.fill(".lock-pin", "4711");
  await app.keys("Enter");
  await app.waitText(".row-subject", /Gehaltsabrechnung Oktober/, 20000);
  // Off again: the PIN goes.
  await openSettings(app, "Sicherheit");
  await clickText(app, "[data-security] [role=radio]", "Aus");
  await app.waitUntil(async () => !(await app.invoke("applock_status")).has_pin, 5000, "lock off");
  await closeSettings();
});

it("encrypts the database while the app runs", async () => {
  assert.equal(header(), "SQLite format 3\0");
  await openSettings(app, "Sicherheit");
  await app.click("[data-encrypt]");
  await app.waitFor("[data-recovery-code]");
  recovery = (await app.text("[data-recovery-code]")).trim();
  assert.match(recovery, /^([A-Z2-7]{4}-){13}[A-Z2-7]{4}$/);
  await app.shot("recovery-key");
  await app.click("[data-recovery-kept] [role=switch]");
  await app.click("[data-encrypt-confirm]");
  await app.waitUntil(async () => (await app.$$("[data-cipher-state=encrypted]")).length > 0, 60000, "encrypted");
  assert.notEqual(header(), "SQLite format 3\0", "the file is no plain SQLite any more");
  const all = fs.readFileSync(path.join(dataDir, "mail.db"));
  assert.ok(!all.includes(Buffer.from("Vertraulicher Inhalt")) && !all.includes(Buffer.from("Gehaltsabrechnung")), "no mail readable in the file");
  await closeSettings();
  await openMessage(app, "Gehaltsabrechnung Oktober");
  // New mail still arrives and is stored encrypted.
  await server.deliver(message({ subject: "Nach der Verschlüsselung", body: "Neu." }));
  await app.invoke("sync_now", { accountId: null });
  await app.waitText(".row-subject", /Nach der Verschlüsselung/, 30000);
});

it("starts with the key from the credential store and drops the plain copy", async () => {
  assert.ok(fs.existsSync(path.join(dataDir, "mail.db.cipher-old")), "the plain copy waits for the next start");
  await app.close();
  app = await launch({ dataDir });
  await app.waitText(".row-subject", /Gehaltsabrechnung Oktober/, 20000);
  assert.ok(!fs.existsSync(path.join(dataDir, "mail.db.cipher-old")), "deleted at the start");
});

it("asks for the recovery key when the key is missing", async () => {
  await app.close();
  const file = path.join(dataDir, "secrets.json");
  const secrets = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.ok(secrets["db-key"], "the key is in the credential store");
  delete secrets["db-key"];
  fs.writeFileSync(file, JSON.stringify(secrets), { mode: 0o600 });
  app = await launch({ dataDir });
  await app.waitFor("[data-key-gate]", 15000);
  await assert.rejects(app.invoke("list_accounts"), /app-locked/);
  await app.shot("recovery-screen");
  await app.fill(".lock-code", "AAAA".repeat(14));
  await app.keys("Enter");
  await app.waitText(".lock-msg", /Tippfehler|typo/, 10000);
  await app.fill(".lock-code", recovery);
  await app.keys("Enter");
  await app.waitText(".row-subject", /Gehaltsabrechnung Oktober/, 30000);
  assert.ok(JSON.parse(fs.readFileSync(file, "utf8"))["db-key"], "remembered on this device");
});

it("removes the encryption again", async () => {
  await openSettings(app, "Sicherheit");
  await app.click("[data-decrypt]");
  await clickText(app, ".dialog .btn-danger", "Verschlüsselung entfernen");
  await app.waitUntil(async () => (await app.$$("[data-cipher-state=plain]")).length > 0, 60000, "decrypted");
  assert.equal(header(), "SQLite format 3\0");
  await closeSettings();
  await app.waitText(".row-subject", /Nach der Verschlüsselung/, 10000);
});

it("warns about a look-alike sender until the user says it is fine", async () => {
  const phish = message({
    from: "PayPal <service@paypa1.com>",
    subject: "Ihr Konto wurde eingeschränkt",
    body: "Bitte bestätigen Sie Ihre Daten.",
    html: '<p>Bitte <a href="https://login.example.ru/s">https://www.paypal.com/signin</a> öffnen.</p>',
    headers: ["Authentication-Results: mx.example.org; spf=fail smtp.mailfrom=paypa1.com; dmarc=fail header.from=paypa1.com"],
  });
  await server.deliver(phish);
  await app.invoke("sync_now", { accountId: null });
  await openMessage(app, "Ihr Konto wurde eingeschränkt", 30000);
  await app.waitFor("[data-phishing]", 10000);
  const reasons = await app.browser.execute(() => Array.from(document.querySelectorAll("[data-phishing] [data-reason]")).map((li) => li.getAttribute("data-reason")));
  assert.deepEqual(reasons.sort(), ["auth_failed", "link", "lookalike"]);
  await app.waitText("[data-phishing]", /paypa1\.com.*paypal\.com/s);
  await app.shot("phishing-warning");
  // Never a block: the message is shown below the warning.
  await app.browser.switchFrame(await app.$(".mail-frame"));
  assert.match(await app.browser.execute(() => document.body.innerText), /Bitte/);
  await app.browser.switchFrame(null);
  await app.click("[data-phishing-ok]");
  await app.waitUntil(async () => (await app.$$("[data-phishing]")).length === 0, 10000, "warning gone");
  // A message from someone the user knows has no warning.
  await openMessage(app, "Gehaltsabrechnung Oktober");
  await sleep(500);
  assert.equal((await app.$$("[data-phishing]")).length, 0);
});
