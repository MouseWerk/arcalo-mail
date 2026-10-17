// Meetings in mail: an invitation as a card with what else is at that time, „Zusagen“ sending
// the iTIP answer to the organizer and filing the appointment, the organizer's cancellation
// taking it out again, „Beitreten“ for an online meeting in a message, and „An Arcalo senden“
// handing the message to Arcalo's command line (a stand-in on the PATH).

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, menuItem, openFolder, openMessage } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

// Arcalo's command line: notes what it was asked and answers as Arcalo does.
const fakeDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-arcalo-"));
const calls = path.join(fakeDir, "calls.jsonl");
fs.writeFileSync(
  path.join(fakeDir, "arcalo"),
  `#!/bin/sh
input=""
if [ "$1" = "pages" ]; then input=$(cat); fi
node -e 'const fs=require("fs");fs.appendFileSync(process.argv[1], JSON.stringify({args: process.argv.slice(3), stdin: process.argv[2]}) + "\\n")' "${calls}" "$input" "$@"
if [ "$1" = "pages" ]; then echo '{"id": 7, "path": "Eingang/Projekt-Kickoff"}'; else echo '{"id": 8, "text": "Projekt-Kickoff"}'; fi
`,
  { mode: 0o755 },
);
const called = () => (fs.existsSync(calls) ? fs.readFileSync(calls, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : []);

const utc = (d) => d.toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");
const tomorrowAt = (h) => {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  d.setHours(h, 0, 0, 0);
  return d;
};

/** An invitation (or its cancellation) from Jonas to Maria. */
function invitation({ method = "REQUEST", uid = "kickoff-1@example.org", sequence = 0, title = "Projekt-Kickoff", start = tomorrowAt(10), end = tomorrowAt(11), subject }) {
  const ics = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Example Corp//Calendar//EN",
    `METHOD:${method}`,
    "BEGIN:VEVENT",
    `UID:${uid}`,
    `SEQUENCE:${sequence}`,
    `DTSTAMP:${utc(new Date())}`,
    `DTSTART:${utc(start)}`,
    `DTEND:${utc(end)}`,
    `SUMMARY:${title}`,
    "LOCATION:https://meet.jit.si/arcalo-kickoff",
    "DESCRIPTION:Wir starten das Projekt.",
    "ORGANIZER;CN=Jonas Berger:mailto:jonas@example.org",
    "ATTENDEE;CN=Maria Muster;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:maria@example.org",
    "ATTENDEE;CN=Lea Wagner;PARTSTAT=ACCEPTED:mailto:lea@example.org",
    ...(method === "CANCEL" ? ["STATUS:CANCELLED"] : []),
    "END:VEVENT",
    "END:VCALENDAR",
    "",
  ].join("\r\n");
  return [
    "From: Jonas Berger <jonas@example.org>",
    "To: Maria Muster <maria@example.org>",
    `Subject: ${subject ?? (method === "CANCEL" ? `Abgesagt: ${title}` : `Einladung: ${title}`)}`,
    `Date: ${new Date().toUTCString().replace("GMT", "+0000")}`,
    `Message-ID: <${Math.random().toString(36).slice(2)}@example.org>`,
    "MIME-Version: 1.0",
    'Content-Type: multipart/alternative; boundary="ALT"',
    "",
    "--ALT",
    "Content-Type: text/plain; charset=utf-8",
    "",
    `${title}: morgen um 10 Uhr.`,
    "--ALT",
    `Content-Type: text/calendar; charset=utf-8; method=${method}`,
    "",
    ics,
    "--ALT--",
    "",
  ].join("\r\n");
}

before(async () => {
  // The stand-in comes first on the PATH the app sees.
  process.env.PATH = `${fakeDir}${path.delimiter}${process.env.PATH}`;
  server = await startServer();
  await server.deliver(invitation({}));
  await server.deliver(
    message({
      from: "Lea Wagner <lea@example.org>",
      subject: "Kurzer Abgleich",
      body: "Hallo Maria,\n\nhier der Link: https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0\n\nLea",
    }),
  );
  app = await launch({ env: { ARCALO_MAIL_TICK_SECS: "2" } });
  await addAccount(app, server);
  // Something else at that time: the card names it.
  await app.invoke("calendar_create", { calendarId: null, fields: { title: "Zahnarzt", start: tomorrowAt(10).toISOString(), end: new Date(tomorrowAt(10).getTime() + 1800e3).toISOString() } });
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
  fs.rmSync(fakeDir, { recursive: true, force: true });
});

it("shows the invitation as a card with what else is at that time", async () => {
  await openMessage(app, "Einladung: Projekt-Kickoff");
  const card = await app.waitFor(".invite-card", 15000);
  const text = await app.textOf(card);
  assert.match(text, /Projekt-Kickoff/);
  assert.match(text, /10:00–11:00/);
  assert.match(text, /Lea Wagner · zugesagt/);
  assert.match(text, /Zur selben Zeit:\s*Zahnarzt/);
  assert.match(text, /Beitreten \(Jitsi\)/);
  await app.shot("meeting-invitation");
});

it("accepts: the organizer gets the iTIP answer, the appointment is filed", async () => {
  const before = (await server.sent()).length;
  await clickText(app, ".invite-answer button span", "Zusagen");
  await app.waitText(".toast-title", /Zugesagt/);
  await app.waitUntil(async () => (await server.sent()).length > before, 20000, "the answer was sent");
  const sent = await server.sent();
  const i = sent.length - 1;
  const raw = await server.sentRaw(i);
  assert.match(raw, /method="?REPLY/i);
  assert.match(raw, /METHOD:REPLY/);
  assert.match(raw, /PARTSTAT=ACCEPTED/);
  assert.match(raw, /UID:kickoff-1@example.org/);
  assert.match(raw, /jonas@example\.org/);
  // The answer shows as the pressed choice; it can still be changed.
  await app.waitText('.invite-answer button[aria-pressed="true"]', /Zusagen/, 10000);
  const from = new Date(Date.now() - 86400e3).toISOString();
  const to = new Date(Date.now() + 3 * 86400e3).toISOString();
  const events = await app.invoke("calendar_events", { from, to });
  assert.ok(events.some((e) => e.title === "Projekt-Kickoff" && e.link === "https://meet.jit.si/arcalo-kickoff"), "filed with its link");
});

it("joins the online meeting", async () => {
  await clickText(app, ".invite-card .join-btn span", "Beitreten (Jitsi)");
  await app.waitUntil(async () => app.opened().some((l) => l.includes("https://meet.jit.si/arcalo-kickoff")), 5000, "the meeting opened in the browser");
  await openMessage(app, "Kurzer Abgleich");
  await clickText(app, ".meeting-row .join-btn span", "Beitreten (Teams)");
  await app.waitUntil(async () => app.opened().some((l) => l.includes("teams.microsoft.com/l/meetup-join")), 5000, "Teams opened");
});

it("shows the appointment in the calendar from the card", async () => {
  await openMessage(app, "Einladung: Projekt-Kickoff");
  await clickText(app, ".invite-card .btn span", "Im Kalender zeigen");
  await app.waitText(".calv-detail .calv-detail-title", /Projekt-Kickoff/, 10000);
  await app.waitText(".calv-detail .calv-join span", /Beitreten \(Jitsi\)/);
  await app.shot("meeting-in-calendar");
  await clickText(app, ".calv-detail .calv-join span", "Beitreten (Jitsi)");
});

it("takes the appointment out when the organizer cancels", async () => {
  await server.deliver(invitation({ method: "CANCEL", sequence: 1 }));
  const from = new Date(Date.now() - 86400e3).toISOString();
  const to = new Date(Date.now() + 3 * 86400e3).toISOString();
  await app.waitUntil(
    async () => !(await app.invoke("calendar_events", { from, to })).some((e) => e.title === "Projekt-Kickoff"),
    30000,
    "the cancelled meeting left the calendar",
  );
});

it("sends a message to Arcalo as a page and as a task", async () => {
  await openFolder(app, "maria@example.org", "Posteingang");
  await openMessage(app, "Kurzer Abgleich");
  await app.click('.reading-pane .toolbar [data-action="more"]');
  await menuItem(app, "Als Seite");
  await app.waitText(".toast-title", /Seite in Arcalo angelegt/, 10000);
  const page = called().find((c) => c.args[0] === "pages");
  assert.deepEqual(page.args.slice(0, 2), ["pages", "create"]);
  assert.ok(page.args.includes("--stdin") && page.args.includes("--json"));
  assert.equal(page.args[page.args.length - 2], "--", "the title comes after --, never read as an option");
  assert.match(page.stdin, /Kurzer Abgleich|Lea Wagner/);
  assert.match(page.stdin, /mid:/, "a link back to the message");
  await app.click('.reading-pane .toolbar [data-action="more"]');
  await menuItem(app, "Als Aufgabe");
  await app.waitText(".toast-title", /Aufgabe in Arcalo angelegt/, 10000);
  const task = called().find((c) => c.args[0] === "tasks");
  assert.deepEqual(task.args.slice(0, 2), ["tasks", "add"]);
});
