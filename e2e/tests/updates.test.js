// Updates of the desktop app against a local feed (lib/update-feed.js, a throwaway key): the
// background download in the mode „Automatisch“ and „Jetzt neu starten“, „Neu in Arcalo Mail“ at
// the next start, a tampered file that is refused, „Installieren“ with its download cancelled and
// continued in the mode „Nur benachrichtigen“, the release page for a copy that cannot install
// itself, and e-mail links (mailto:) from another app with Arcalo Mail as the default mail app.
// The debug build takes the feed, the key and the version from the environment and notes the
// install in updates/fake-install.json instead of running it (src-tauri/src/updates.rs).

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { APP, launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, openSettings } from "../lib/mail.js";
import { startFeed, throwawayKey } from "../lib/update-feed.js";

let app;
let feed;
let server;
const it = guarded(test, () => app);
const key = throwawayKey();
const NOTES = "## Arcalo Mail 1.0.1\n\n## Improved\n\n- Search answers faster.\n\n## Checklist for Windows (manual)\n\n- [ ] Install over 1.0.0";
const env = (over = {}) => ({
  ARCALO_MAIL_UPDATE_ENDPOINT: feed.url,
  ARCALO_MAIL_UPDATE_PUBKEY: key.pubkey,
  ARCALO_MAIL_UPDATE_CURRENT: "1.0.0",
  ARCALO_MAIL_UPDATE_FAKE_INSTALL: "1",
  ...over,
});
const fakeInstall = (dir) => path.join(dir, "updates", "fake-install.json");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

before(async () => {
  feed = await startFeed({ key, version: "1.0.1", notes: NOTES });
  server = await startServer();
});

after(async () => {
  await app?.close();
  await feed?.close();
  server?.stop();
});

/** A test's app is closed after it, also when it failed (a second app would only bring the
 *  first one to the front). */
async function closeApp() {
  await app?.close().catch(() => {});
  app = null;
}

/** The app ends itself for the (fake) install: the session goes with it. */
async function closeAfterInstall(dir) {
  await app.waitUntil(async () => fs.existsSync(fakeInstall(dir)), 20000, "the update was not installed");
  await app.close().catch(() => {});
  app = null;
}

it("downloads in the background, installs with „Jetzt neu starten“ and shows what is new", async () => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-update-"));
  try {
    app = await launch({ env: env(), dataDir });
    await addAccount(app, server);
    await app.browser.refresh();
    await app.waitFor(".account");
    await app.invoke("update_check", { manual: true });
    await app.waitText(".update-card-title", /Update 1\.0\.1 bereit/, 20000);
    assert.match(await app.text(".update-card"), /beim Beenden von Arcalo Mail installiert/);
    assert.ok(fs.existsSync(path.join(dataDir, "updates", "1.0.1.update")), "downloaded and verified");
    await clickText(app, ".update-card .btn", "Neuigkeiten");
    await app.waitText(".dialog .update-notes", /Search answers faster/);
    assert.doesNotMatch(await app.text(".dialog .update-notes"), /Checklist|Install over/, "the testers' checklist is left out");
    await app.shot("update-notes");
    await app.keys("Escape");
    await app.shot("update-ready");
    await clickText(app, ".update-card .btn", "Jetzt neu starten");
    await closeAfterInstall(dataDir);
    const done = JSON.parse(fs.readFileSync(fakeInstall(dataDir), "utf8"));
    assert.deepEqual(done, { version: "1.0.1", from: "1.0.0", restart: true });

    // The next start runs 1.0.1: „Neu in Arcalo Mail“ once, with the notes of the feed.
    app = await launch({ env: env({ ARCALO_MAIL_UPDATE_CURRENT: "1.0.1" }), dataDir });
    await app.waitText(".dialog-title", /Neu in Arcalo Mail 1\.0\.1/, 15000);
    assert.match(await app.text(".dialog .update-notes"), /Search answers faster/);
    await app.shot("update-whats-new");
    await clickText(app, ".dialog .btn-primary", "Fertig");
    const status = await app.invoke("update_status");
    assert.equal(status.whats_new_seen, "1.0.1");
    assert.equal(status.restarted, null, "reported once");
  } finally {
    await closeApp();
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
});

it("refuses a tampered file, and „Installieren“ can be cancelled and continued", async () => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-update-"));
  try {
    app = await launch({ env: env(), dataDir });
    await addAccount(app, server);
    await app.browser.refresh();
    await app.waitFor(".account");
    await openSettings(app, "Über Arcalo Mail");
    await app.waitFor('[data-updates="desktop"]');
    await clickText(app, '[data-updates="desktop"] .segmented button', "Nur benachrichtigen");
    await app.waitUntil(async () => (await app.invoke("get_settings")).updates === "notify", 5000, "mode not saved");
    await app.click('.settings-head [aria-label="Schließen"]');

    feed.mode = "tampered";
    await app.invoke("update_check", { manual: true });
    await app.waitText(".update-card-title", /Arcalo Mail 1\.0\.1 ist verfügbar/, 15000);
    assert.ok(!fs.existsSync(path.join(dataDir, "updates", "1.0.1.part")), "nothing downloaded before the click");
    await clickText(app, ".update-card .btn", "Installieren");
    await app.waitText(".toast-title", /Update nicht installiert/, 20000);
    await app.waitText(".toast-detail", /Signatur des Updates ist ungültig/, 2000);
    assert.ok(!fs.existsSync(fakeInstall(dataDir)));

    feed.mode = "throttle";
    feed.ranges = [];
    await clickText(app, ".update-card .btn", "Installieren");
    await app.waitText(".update-card-title", /wird geladen/, 10000);
    await app.waitUntil(async () => /\d+ %/.test(await app.text(".update-card-detail")), 10000, "no progress");
    await app.shot("update-downloading");
    await clickText(app, ".update-card .btn", "Abbrechen");
    await app.waitText(".update-card-title", /ist verfügbar/, 10000);
    const part = path.join(dataDir, "updates", "1.0.1.part");
    assert.ok(fs.existsSync(part) && fs.statSync(part).size > 0, "the part stays");
    assert.ok(!fs.existsSync(fakeInstall(dataDir)), "cancelled: nothing installed");

    feed.mode = "ok";
    await clickText(app, ".update-card .btn", "Installieren");
    await closeAfterInstall(dataDir);
    assert.ok(feed.ranges.some((r) => r > 0), `continued where it stopped: ${feed.ranges}`);
    assert.equal(JSON.parse(fs.readFileSync(fakeInstall(dataDir), "utf8")).version, "1.0.1");
  } finally {
    await closeApp();
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
});

it("a copy that cannot install itself says why and offers the release page", async () => {
  try {
    app = await launch({ env: env({ ARCALO_MAIL_UPDATE_MANUAL: "portable" }) });
    await addAccount(app, server);
    await app.browser.refresh();
    await app.waitFor(".account");
    await app.invoke("update_check", { manual: true });
    await app.waitText(".update-card-title", /Arcalo Mail 1\.0\.1 ist verfügbar/, 15000);
    assert.match(await app.text(".update-card"), /portable Version installiert Updates nicht selbst/);
    await app.shot("update-portable");
    await clickText(app, ".update-card .btn", "Release-Seite öffnen");
    await app.waitUntil(async () => app.opened().some((l) => l.includes("releases/tag/v1.0.1")), 5000, "release page not opened");
    const dir = app.dataDir;
    assert.ok(!fs.existsSync(path.join(dir, "updates", "1.0.1.part")), "nothing downloaded");
  } finally {
    await closeApp();
  }
});

it("an e-mail link of another app opens the compose sheet, and the app can become the default mail app", async () => {
  app = await launch({ env: env() });
  await addAccount(app, server);
  await app.browser.refresh();
  await app.waitFor(".account");
  await openSettings(app, "Über Arcalo Mail");
  await app.waitFor("[data-default-mail]");
  await clickText(app, "[data-default-mail] .btn", "Als Standard-Mail-App festlegen");
  await app.waitText("[data-default-mail] .set-badge", /Standard/, 10000);
  const entry = path.join(app.dataDir, "xdg-data", "applications", "de.mousewerk.arcalo.mail.desktop");
  assert.match(fs.readFileSync(entry, "utf8"), /MimeType=x-scheme-handler\/mailto;/);
  assert.match(fs.readFileSync(entry, "utf8"), new RegExp(`Exec="${APP.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}" %u`));
  await app.shot("default-mail-app");
  await app.click('.settings-head [aria-label="Schließen"]');

  // A second start with the address (what the system runs for a click on a mailto: link)
  // hands it to the running app and ends.
  const link = "mailto:jonas@example.org?cc=lea@example.org&subject=Gr%C3%BC%C3%9Fe%20aus%20Berlin&body=Hallo%20Jonas";
  const second = spawn(APP, [link], { env: { ...process.env, ...app.env }, stdio: "ignore" });
  const ended = await Promise.race([new Promise((r) => second.on("exit", () => r(true))), sleep(15000).then(() => false)]);
  if (!ended) second.kill();
  assert.ok(ended, "the second start ended after handing over the address");
  await app.waitFor(".compose", 10000);
  assert.equal(await (await app.$("#compose-subject")).getValue(), "Grüße aus Berlin");
  const chips = [];
  for (const c of await app.$$(".compose .chip-text")) chips.push(await app.textOf(c));
  assert.ok(chips.includes("jonas@example.org") && chips.includes("lea@example.org"), chips.join(", "));
  await app.waitUntil(async () => (await app.browser.execute(() => document.querySelector(".compose")?.innerText ?? "")).includes("Hallo Jonas"), 5000, "body");
  await app.shot("mailto-compose");
});
