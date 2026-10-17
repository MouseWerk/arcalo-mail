// The calendar against the test server's CalDAV calendars (`work`, `home`): a server added in
// the settings, an appointment another client put there, a new one made with a double click and
// written to the server, moved with the keyboard and taken back with „Rückgängig“, deleted and
// brought back, a calendar hidden in the legend, and the views.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, openSettings, openView } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const caldavState = async () => (await fetch(`http://127.0.0.1:${server.control}/caldav-state`)).json();
const putEvent = (calendar, name, ics) => fetch(`http://127.0.0.1:${server.control}/caldav-event?calendar=${calendar}&name=${name}`, { method: "POST", body: ics }).then((r) => r.text());

/** `20261014T090000Z` of a Date. */
const utc = (d) => d.toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");
/** Today at `h` o'clock local time. */
const todayAt = (h, m = 0) => {
  const d = new Date();
  d.setHours(h, m, 0, 0);
  return d;
};
const vevent = (uid, title, start, end) =>
  ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Other client//EN", "BEGIN:VEVENT", `UID:${uid}`, `DTSTAMP:${utc(new Date())}`, `DTSTART:${utc(start)}`, `DTEND:${utc(end)}`, `SUMMARY:${title}`, "LOCATION:Raum 4", "END:VEVENT", "END:VCALENDAR", ""].join("\r\n");

/** The objects on the server whose data matches `pattern`. */
async function onServer(pattern) {
  const s = await caldavState();
  return Object.values(s.calendars).flatMap((c) => Object.values(c)).filter((d) => pattern.test(d));
}

async function waitServer(pattern, present = true, timeout = 20000) {
  await app.waitUntil(async () => ((await onServer(pattern)).length > 0) === present, timeout, `${pattern} ${present ? "on" : "gone from"} the server`);
}

/** The grid's appointment with this title. */
async function event(title) {
  await app.waitText(".calv-ev .calv-ev-title", new RegExp(title), 15000);
  for (const e of await app.$$(".calv-ev")) if ((await app.textOf(e)).includes(title)) return e;
  throw new Error(`no appointment ${title}`);
}

/** Waits until the grid's appointment `title` says `time` (its label has the time). */
async function waitLabel(title, time) {
  await app.waitUntil(
    async () => {
      for (const e of await app.$$(".calv-ev")) {
        const label = (await e.getAttribute("aria-label")) ?? "";
        if (label.includes(title) && time.test(label)) return true;
      }
      return false;
    },
    10000,
    `${title} at ${time}`,
  );
}

/** Waits until no toast is shown (an older one's „Rückgängig“ must not be clicked). */
async function noToasts() {
  await app.waitUntil(async () => (await app.$$(".toast")).length === 0, 12000, "toasts gone");
}

before(async () => {
  server = await startServer();
  await putEvent("work", "planung.ics", vevent("planung-1@other", "Planungsrunde", todayAt(9), todayAt(10)));
  app = await launch({ env: { ARCALO_MAIL_TICK_SECS: "2" } });
  await addAccount(app, server);
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("adds a CalDAV server in the settings and shows its appointments", async () => {
  await openSettings(app, "Kalender");
  await clickText(app, ".set-section .btn span", "CalDAV-Kalender hinzufügen");
  await app.waitFor(".dialog");
  // „Anderer Server“: the test server's address.
  const select = await app.$(".dialog select");
  await app.browser.execute((s) => {
    s.value = String(s.options.length - 1);
    s.dispatchEvent(new Event("change", { bubbles: true }));
  }, select);
  await app.fill(".dialog .stack-field:nth-of-type(2) input", `http://127.0.0.1:${server.control}/caldav/`);
  await app.fill(".dialog .stack-field:nth-of-type(3) input", server.user);
  await app.fill('.dialog input[type="password"]', server.password);
  await clickText(app, ".dialog .btn-primary span", "Verbinden");
  await app.waitUntil(async () => (await app.$$(".dialog")).length === 0, 20000, "dialog closed");
  await app.waitText(".calset-cal-name", /Arbeit|work/i, 15000);
  await app.shot("calendar-settings");
  await app.keys("Escape");
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
  await openView(app, "Kalender");
  await app.waitFor(".calv");
  const e = await event("Planungsrunde");
  assert.match(await app.textOf(e), /09:00–10:00/);
});

it("creates an appointment with a double click and writes it to the server", async () => {
  await clickText(app, ".calv-views .segmented button", "Tag");
  const col = await app.waitFor(".calv-col.today");
  // 14:00 in the day's column (48 px per hour).
  await app.browser.execute((c) => {
    const r = c.getBoundingClientRect();
    c.closest(".calv-scroll").scrollTop = 12 * 48;
    const y = c.getBoundingClientRect().top + 14 * 48 + 5;
    c.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, clientX: r.left + 20, clientY: y }));
  }, col);
  const name = await app.waitFor(".calv-entry-editor .calv-entry-name");
  await name.click();
  await app.type("Zahnarzt");
  await app.keys("Enter");
  const e = await event("Zahnarzt");
  assert.match(await e.getAttribute("aria-label"), /14:00–14:30/);
  await waitServer(/SUMMARY:Zahnarzt/);
  await app.shot("calendar-created");
});

it("moves it with the keyboard, and „Rückgängig“ takes it back", async () => {
  await noToasts();
  await app.browser.execute((x) => x.focus(), await event("Zahnarzt"));
  await app.keys("ArrowDown");
  await waitLabel("Zahnarzt", /14:15–14:45/);
  const moved = utc(todayAt(14, 15)).slice(0, 13);
  await app.waitUntil(async () => (await onServer(/SUMMARY:Zahnarzt/)).some((d) => new RegExp(`DTSTART[^\r\n]*${moved}`).test(d)), 20000, "moved on the server");
  await clickText(app, ".toast-action", "Rückgängig");
  await waitLabel("Zahnarzt", /14:00–14:30/);
  const start = utc(todayAt(14)).slice(0, 13);
  await app.waitUntil(async () => (await onServer(/SUMMARY:Zahnarzt/)).some((d) => new RegExp(`DTSTART[^\r\n]*${start}`).test(d)), 20000, "back at 14:00 on the server");
});

it("deletes it and brings it back", async () => {
  await noToasts();
  await app.browser.execute((x) => x.focus(), await event("Zahnarzt"));
  await app.keys("Delete");
  await app.waitUntil(async () => !(await app.text(".calv-grid")).includes("Zahnarzt"), 8000, "gone from the grid");
  await waitServer(/SUMMARY:Zahnarzt/, false);
  await clickText(app, ".toast-action", "Rückgängig");
  await event("Zahnarzt");
  await waitServer(/SUMMARY:Zahnarzt/);
});

it("hides a calendar in the legend and switches views", async () => {
  await clickText(app, ".calv-views .segmented button", "Arbeitswoche");
  const legend = await app.$$(".calv-legend-item");
  assert.ok(legend.length >= 2, "a legend with the calendars");
  let work = null;
  for (const l of legend) if (/Arbeit|work/i.test(await app.textOf(l))) work = l;
  assert.ok(work);
  await work.click();
  await app.waitUntil(async () => !(await app.text(".calv-grid")).includes("Planungsrunde"), 8000, "hidden");
  await work.click();
  await event("Planungsrunde");
  await clickText(app, ".calv-views .segmented button", "Monat");
  await app.waitText(".calv-mev .calv-ev-title", /Planungsrunde/);
  await clickText(app, ".calv-views .segmented button", "Liste");
  await app.waitText(".calv-agenda-row .calv-ev-title", /Planungsrunde/);
  await clickText(app, ".calv-views .segmented button", "Woche");
  await event("Planungsrunde");
  await sleep(200);
});

it("takes a change another client made", async () => {
  await putEvent("home", "lauf.ics", vevent("lauf-1@other", "Laufgruppe", todayAt(18), todayAt(19)));
  await app.click('.calv-actions [aria-label="Kalender jetzt synchronisieren"]');
  await event("Laufgruppe");
});
