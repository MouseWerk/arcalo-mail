// The store screenshots (not part of the test suite): the desktop app in its touch layout
// (ARCALO_MAIL_TOUCH=1) at the sizes Google Play and the App Store ask for, drawn at the device's
// pixel ratio (GDK_SCALE), with the sample mailboxes of the README pictures on the fixed clock:
//
//   play-phone   360 × 640 at 3x  = 1080 × 1920  (Google Play phone, 9:16)
//   iphone-6.9   440 × 956 at 3x  = 1320 × 2868  (App Store, iPhone 6.9")
//   iphone-6.5   428 × 926 at 3x  = 1284 × 2778  (App Store, iPhone 6.5")
//   ipad-13     1032 × 1376 at 2x = 2064 × 2752  (App Store, iPad 13"; Play's tablets)
//
// Every picture shows the whole window. Run after building the app and the test server:
//   cd e2e && npm run shots:store
// ARCALO_MAIL_SHOTS sets the folder (default docs/store/screenshots).

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch } from "../lib/harness.js";
import { startServer } from "../lib/server.js";
import { addAccount } from "../lib/server.js";
import * as mail from "../lib/mail.js";
import { NOW, PERSONAL, WORK, fill, personalMail, workMail } from "./mailbox.js";

const OUT = process.env.ARCALO_MAIL_SHOTS ?? path.resolve(import.meta.dirname, "../../docs/store/screenshots");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
for (const k of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
  delete process.env[k];
  delete process.env[k.toLowerCase()];
}

const SIZES = [
  { name: "play-phone", width: 360, height: 640, scale: 3 },
  { name: "iphone-6.9", width: 440, height: 956, scale: 3 },
  { name: "iphone-6.5", width: 428, height: 926, scale: 3 },
  { name: "ipad-13", width: 1032, height: 1376, scale: 2 },
];

let work, personal, app, dataDir;

/** `20260312T103000Z` of a day of the screenshots' week (0 = Monday 9 March 2026) at `hh:mm` UTC. */
const when = (day, hh, mm = 0) => new Date(Date.UTC(2026, 2, 9 + day, hh, mm)).toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");
const vcal = (uid, title, start, end, extra = []) =>
  ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Example//Calendar//EN", "BEGIN:VEVENT", `UID:${uid}`, "DTSTAMP:20260301T090000Z", `DTSTART:${start}`, `DTEND:${end}`, `SUMMARY:${title}`, ...extra, "END:VEVENT", "END:VCALENDAR", ""].join("\r\n");

const EVENTS = [
  ["work", "standup", vcal("standup@example.com", "Standup", when(0, 9, 30), when(0, 9, 45), ["RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=20"])],
  ["work", "review", vcal("review@example.com", "Design review", when(0, 13), when(0, 14), ["LOCATION:Studio 2"])],
  ["work", "lumen", vcal("lumen@example.com", "Customer interview: Lumen", when(1, 11), when(1, 12, 30), ["LOCATION:https://teams.microsoft.com/l/meetup-join/19%3ameeting_lumen"])],
  ["work", "workshop", vcal("workshop@example.com", "Workshop: onboarding flow", when(2, 14), when(2, 16), ["LOCATION:Studio 1"])],
  ["home", "climbing", vcal("climbing@example.net", "Climbing", when(2, 18, 30), when(2, 20))],
  ["work", "q2", vcal("q2-planning@example.com", "Q2 launch planning", when(3, 10, 30), when(3, 11, 30), ["LOCATION:https://meet.jit.si/q2-launch-planning"])],
  ["home", "lunch", vcal("lunch@example.net", "Lunch with Ana", when(3, 12, 30), when(3, 13, 30), ["LOCATION:Café Lindgren"])],
  ["work", "debrief", vcal("debrief@example.com", "Interview debrief", when(3, 16), when(3, 17))],
  ["work", "demo", vcal("demo@example.com", "Sprint demo", when(4, 11), when(4, 12))],
  ["home", "dinner", vcal("dinner@example.net", "Dinner with Leo", when(4, 19), when(4, 21, 30))],
];

async function ready() {
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
}

async function setTheme(theme) {
  const s = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...s, theme } });
  await app.browser.refresh();
  await ready();
}

/** A picture of the whole window, without toasts or a focus ring; checks its size in pixels. */
async function shot(size, name) {
  await app.browser.execute(() => document.querySelectorAll(".toast .icon-btn").forEach((b) => b.click()));
  await app.browser.execute(() => document.activeElement?.blur());
  await sleep(600);
  const dir = path.join(OUT, size.name);
  await app.shot(name, dir);
  const png = fs.readFileSync(path.join(dir, `${name}.png`));
  assert.deepEqual([png.readUInt32BE(16), png.readUInt32BE(20)], [size.width * size.scale, size.height * size.scale], `${size.name}/${name}`);
}

/** Back from a message to the list (phone), once the list has slid in. */
async function back() {
  await app.browser.back();
  await app.waitUntil(async () => (await app.browser.execute(() => document.querySelector(".app")?.dataset.view)) !== "message", 5000, "back to the list");
  await sleep(500);
}

const layout = () => app.browser.execute(() => document.querySelector(".app")?.dataset.layout);

/** The inbox of the work account (phone: the list; tablet: the list beside the message). */
async function inbox() {
  if ((await layout()) === "phone" && (await app.browser.execute(() => document.querySelector(".app")?.dataset.view === "message"))) await back();
  await app.click(".folders-btn");
  await mail.openFolder(app, "Work", "Inbox");
  await app.waitFor(".list-pane");
}

before(async () => {
  work = await startServer({ user: WORK, password: "sample-password" });
  personal = await startServer({ user: PERSONAL, password: "sample-password" });
  await fill(work, workMail());
  await fill(personal, personalMail());
  for (const [cal, name, ics] of EVENTS) await fetch(`http://127.0.0.1:${work.control}/caldav-event?calendar=${cal}&name=${name}.ics`, { method: "POST", body: ics });
});

after(async () => {
  await app?.close().catch(() => {});
  work?.stop();
  personal?.stop();
  if (dataDir) fs.rmSync(dataDir, { recursive: true, force: true });
});

for (const size of SIZES) {
  test(`store screenshots, ${size.name}`, async () => {
    dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-store-shots-"));
    const env = { ARCALO_MAIL_NOW: String(NOW), TZ: "Europe/London", ARCALO_MAIL_TOUCH: "1", GDK_SCALE: String(size.scale) };
    app = await launch({ width: size.width, height: size.height, locale: "en-GB", env, dataDir });
    try {
      await addAccount(app, work, { name: "Work", sender: "Emma Larsen" });
      await addAccount(app, personal, { name: "Personal", sender: "Emma Larsen" });
      await app.invoke("caldav_add", { name: "Example Cloud", url: `http://127.0.0.1:${work.control}/caldav/`, username: WORK, password: "sample-password" });
      for (const c of (await app.invoke("calendar_overview")).calendars) {
        if (c.name === "Arbeit") await app.invoke("calendar_set", { id: c.id, patch: { name: "Work", color: "#2563eb" } });
        if (c.name === "Privat") await app.invoke("calendar_set", { id: c.id, patch: { name: "Personal", color: "#0d9488" } });
        if (c.kind === "local") await app.invoke("calendar_set", { id: c.id, patch: { hidden: true } });
      }
      await setTheme("light");
      await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 6, 30000, "first lines in the list");
      const tablet = size.name.startsWith("ipad");
      assert.equal(await layout(), tablet ? "tablet" : "phone");

      // 1: the sorted inbox (the tablet with a conversation beside it).
      await inbox();
      if (tablet) await mail.openMessage(app, "Q2 launch plan");
      await shot(size, "1-inbox");

      // 2: a conversation (phone) / the calendar (tablet).
      if (!tablet) {
        await mail.openMessage(app, "Q2 launch plan");
        await shot(size, "2-conversation");
        await back();
      }

      // 3: the calendar's week (tablet) or day (phone), on the day of the meeting.
      await app.click(".folders-btn");
      await mail.openView(app, "Calendar");
      if (tablet) await mail.clickText(app, ".calv-views .segmented button", "Week");
      else await mail.choose(app, ".calv-view-select", "day");
      await app.waitText(".calv-ev .calv-ev-title", /Q2 launch planning|Standup/, 20000);
      await app.browser.execute(() => {
        const s = document.querySelector(".calv-scroll");
        if (s) s.scrollTop = 8.5 * 48;
      });
      await shot(size, tablet ? "2-calendar" : "3-calendar");

      // 4: writing a reply.
      await inbox();
      await mail.openTab(app, "receipts");
      await mail.openMessage(app, "Invoice INV-2026-031");
      await app.click('.touch-actions [aria-label="Reply"]');
      await app.waitFor(".compose");
      await app.browser.execute(() => {
        const ed = document.querySelector(".compose .editor");
        ed.focus();
        const r = document.createRange();
        r.setStart(ed.firstChild ?? ed, 0);
        r.collapse(true);
        getSelection().removeAllRanges();
        getSelection().addRange(r);
      });
      await app.browser.execute(() => document.execCommand("insertText", false, "Thanks Daniel, approved for the budget review."));
      await shot(size, tablet ? "3-compose" : "4-compose");
      await mail.discardCompose(app, "Discard");

      // 5: the inbox in the dark theme.
      await setTheme("dark");
      await inbox();
      if (tablet) await mail.openMessage(app, "Q2 launch plan");
      await shot(size, tablet ? "4-inbox-dark" : "5-inbox-dark");
    } finally {
      await app.close().catch(() => {});
      app = null;
      fs.rmSync(dataDir, { recursive: true, force: true });
      dataDir = null;
    }
  });
}
