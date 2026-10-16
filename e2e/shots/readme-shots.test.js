// Screenshots for the README (not part of the test suite): the app in English with the sample
// mailboxes of shots/mailbox.js, served by two test servers on this computer, on a fixed clock.
// Every picture shows the whole window. Run after building the app and the test server:
//   cd e2e && npm run shots
// ARCALO_MAIL_SHOTS sets the folder (default docs/screenshots), ARCALO_MAIL_SCENES=inbox,dark,…
// takes only some scenes (names in the tests below; every scene after "inbox" needs it).

import { test, before, after } from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import * as mail from "../lib/mail.js";
import { NOW, PERSONAL, WORK, fill, personalMail, workMail } from "./mailbox.js";

const OUT = process.env.ARCALO_MAIL_SHOTS ?? path.resolve(import.meta.dirname, "../../docs/screenshots");
const ONLY = process.env.ARCALO_MAIL_SCENES?.split(",") ?? null;
const scene = (name, fn) => test(name, { skip: Boolean(ONLY && !ONLY.includes(name)) }, fn);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// No proxy of the machine that makes the pictures shows up in Settings → Network (the app gets
// this process's environment).
for (const k of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
  delete process.env[k];
  delete process.env[k.toLowerCase()];
}
const ENV = { ARCALO_MAIL_NOW: String(NOW), TZ: "Europe/London" };

let work, personal, app, dataDir;

/** The window as it is, after the UI settled; toasts of earlier steps are closed first. */
async function shot(name) {
  await app.browser.execute(() => document.querySelectorAll(".toast .icon-btn").forEach((b) => b.click()));
  await sleep(500);
  await app.shot(name, OUT);
}

async function setTheme(theme) {
  const s = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...s, theme } });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
}

const clickText = (sel, text) => mail.clickText(app, sel, text);
const openFolder = (account, name) => mail.openFolder(app, account, name);
const openMessage = (subject) => mail.openMessage(app, subject);

before(async () => {
  work = await startServer({ user: WORK, password: "sample-password" });
  personal = await startServer({ user: PERSONAL, password: "sample-password" });
  await fill(work, workMail());
  await fill(personal, personalMail());
  dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-shots-"));
});

after(async () => {
  await app?.close().catch(() => {});
  work?.stop();
  personal?.stop();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

scene("setup", async () => {
  // The first start: the servers come from the provider's autoconfig file; the check is slowed
  // down so the picture shows it.
  const env = { ...ENV, ARCALO_MAIL_AUTOCONFIG: `http://127.0.0.1:${work.control}/autoconfig?host=localhost` };
  app = await launch({ locale: "en-GB", env });
  await work.delay(30000);
  await app.fill(".setup-fields input[autocomplete=name]", "Emma Larsen");
  await app.fill(".setup-fields input[type=email]", WORK);
  await app.fill(".setup-fields input[type=password]", "sample-password");
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".setup-found", 15000);
  await app.waitFor(".setup-progress");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("setup");
  await app.close();
  app = null;
  await work.delay(0);
});

scene("setup-microsoft", async () => {
  // Signing in with Microsoft: the app waits for the browser (nobody follows the address here).
  app = await launch({ locale: "en-GB", env: { ...ENV, ...work.microsoftEnv() } });
  await app.fill(".setup-fields input[autocomplete=name]", "Emma Larsen");
  await app.fill(".setup-fields input[type=email]", "emma@northwind.example");
  await clickText(".setup-other .btn", "Sign in with Microsoft");
  await app.waitFor(".setup-microsoft .setup-progress");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("setup-microsoft");
  await app.close();
  app = null;
});

scene("setup-gmail", async () => {
  // A Gmail address: the guide to an app password.
  app = await launch({ locale: "en-GB", env: ENV });
  await app.fill(".setup-fields input[autocomplete=name]", "Emma Larsen");
  await app.fill(".setup-fields input[type=email]", "name@gmail.com");
  await app.waitFor(".guide");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("setup-gmail");
  await app.close();
  app = null;
});

scene("inbox", async () => {
  app = await launch({ locale: "en-GB", env: ENV, dataDir });
  await addAccount(app, work, { name: "Work", sender: "Emma Larsen" });
  await addAccount(app, personal, { name: "Personal", sender: "Emma Larsen" });
  await setTheme("light");
  // The bodies come ahead after the first sync: the list shows first lines.
  await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 8, 30000, "first lines in the list");
  await openMessage("Q2 launch plan");
  await shot("inbox-light");
});

scene("dark", async () => {
  await setTheme("dark");
  await openMessage("Q2 launch plan");
  await shot("inbox-dark");
  await setTheme("light");
});

scene("html", async () => {
  await openMessage("Field Notes #48");
  await app.waitFor(".reading-pane .notice");
  await shot("reading-html");
});

scene("compose", async () => {
  // Forwarding the invoice: its PDF goes along.
  await openMessage("Invoice INV-2026-031");
  await clickText(".reading-pane .toolbar .btn", "Forward");
  await app.waitFor(".compose .compose-attachments");
  await app.click(".compose .address-field .chip-input");
  await app.type("Daniel");
  await app.waitFor(".compose [role=option]");
  await app.keys("Enter");
  await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor");
    ed.focus();
    const r = document.createRange();
    r.setStart(ed.firstChild ?? ed, 0);
    r.collapse(true);
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  });
  await app.type("Hi Daniel, here is the March invoice for the budget review on Monday.");
  await shot("compose");
  await app.click('.compose-foot [aria-label="Discard"]');
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
});

scene("search", async () => {
  await openFolder("Work", "Inbox");
  await app.fill(".search-input", "launch");
  await app.waitText(".list-title", /Search/);
  await app.waitUntil(async () => (await app.$$(".row")).length >= 1, 10000, "results");
  await openMessage("Q2 launch plan");
  await shot("search");
  await app.click(".search-clear");
});

scene("settings", async () => {
  await app.click(".sidebar-head .icon-btn");
  await app.waitFor(".settings-nav");
  await clickText(".settings-nav-item", "Appearance");
  await shot("settings");
  await app.keys("Escape");
});

scene("contrast", async () => {
  await setTheme("contrast");
  await openMessage("Photos from the offsite");
  await shot("contrast");
  await setTheme("light");
});

scene("palette", async () => {
  await openFolder("Work", "Inbox");
  await app.keys(["Control", "k"]);
  await app.waitFor(".palette");
  await app.type("go");
  await shot("palette");
  await app.keys("Escape");
});

scene("network", async () => {
  await app.click(".sidebar-head .icon-btn");
  await app.waitFor(".settings-nav");
  await clickText(".settings-nav-item", "Network");
  await shot("network");
  await app.keys("Escape");
});

scene("narrow", async () => {
  await app.browser.setWindowSize(900, 720);
  await openFolder("Personal", "Inbox");
  await openMessage("Lunch on Friday?");
  await shot("narrow");
});
