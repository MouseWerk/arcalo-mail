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
import { portrait } from "../lib/png.js";

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
// The unsubscribe addresses of the sample newsletters are the test server on this computer,
// which mail may not point at otherwise (as in tests/sorting.test.js).
const ENV = { ARCALO_MAIL_NOW: String(NOW), TZ: "Europe/London", ARCALO_MAIL_TEST_LOOPBACK: "1" };

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
  await fill(work, workMail(work.webUrl));
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

scene("tabs", async () => {
  // The work inbox sorted: people first, a pinned message on top, the other tabs with counts.
  await openFolder("Work", "Inbox");
  await openMessage("Customer interviews: notes");
  await app.keys("p");
  await mail.waitRow(app, "Customer interviews: notes");
  await openMessage("Q2 launch plan");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("inbox-tabs");
});

scene("html", async () => {
  // A newsletter's issues as one stream: remote images blocked, the tracking pixel removed.
  await mail.openTab(app, "newsletters");
  await mail.waitRow(app, "Field Notes #48: Designing for slow software");
  for (const r of await app.$$(".row")) if ((await app.textOf(r)).includes("Field Notes")) await r.click();
  await app.waitFor(".reading-pane .notice");
  await app.waitFor(".tracker-note");
  await shot("reading-html");
});

scene("subscriptions", async () => {
  await clickText(".list-title-tools .btn span", "Subscriptions");
  await app.waitText(".subscription-item", /Design Weekly/);
  for (const item of await app.$$(".subscription-item")) {
    if ((await app.textOf(item)).includes("Design Weekly")) await (await item.$(".btn")).click();
  }
  await clickText(".dialog-foot .btn-primary", "Unsubscribe");
  await app.waitText(".subscription-item", /Unsubscribed/);
  await shot("subscriptions");
});

scene("compose", async () => {
  // Forwarding the invoice: its PDF goes along.
  await openFolder("Work", "Inbox");
  await mail.openTab(app, "receipts");
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

scene("snooze", async () => {
  await openFolder("Work", "Inbox");
  await openMessage("Draft for the case study");
  await app.click('.reading-pane .toolbar [data-action="snooze"]');
  await app.waitFor(".menu");
  await shot("snooze-menu");
  await app.keys("Escape");
});

scene("screener", async () => {
  // The Screener switched on two days ago: who wrote for the first time since waits.
  const s = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...s, screener: true, screener_since: NOW - 2 * 86400 } });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
  await mail.openView(app, "New senders");
  await app.waitText(".screener-item", /Sofia Marin/);
  for (const item of await app.$$(".screener-item")) if ((await app.textOf(item)).includes("Sofia Marin")) await (await item.$(".smart-main")).click();
  await app.waitText(".message-subject", /Lunch on Friday/);
  await shot("screener");
  const back = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...back, screener: false, screener_since: null } });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
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

/** The assistant with a model on this computer (the work server's fake OpenAI-compatible
 *  provider stands in for Ollama). */
async function assistantOn() {
  const s = await app.invoke("get_settings");
  if (s.ai.providers.length) return;
  s.ai.providers = [{ id: "ollama", name: "Ollama", kind: "ollama", base_url: `http://127.0.0.1:${work.control}/ai`, model: "llama3.2", api_version: "" }];
  s.ai.default_provider = "ollama";
  await app.invoke("set_settings", { settings: s });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
}

const answer = (task, text) => fetch(`http://127.0.0.1:${work.control}/ai-answer?task=${task}`, { method: "POST", body: text });

scene("ai-summary", async () => {
  await answer(
    "summary",
    [
      "- Daniel shared the first Q2 launch plan: beta of the onboarding flow from 1 April, public launch with the pricing page on 6 May.",
      "- You agreed if the onboarding screens are frozen by 20 March, after one more round with two beta teams.",
      "- Priya booked those sessions for Tuesday and Wednesday and brings the empty states and the welcome copy to Friday's review.",
      "- Open: confirm the freeze date before the review on Friday.",
    ].join("\n"),
  );
  await assistantOn();
  await openFolder("Work", "Inbox");
  await openMessage("Q2 launch plan");
  await app.click('[data-ai="summary"]');
  await app.waitText(".assistant-out .ai-text", /Open: confirm the freeze date/, 20000);
  await app.waitUntil(async () => (await app.$$(".assistant-out .ai-caret")).length === 0, 10000, "the stream ends");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("ai-summary");
});

scene("writing-help", async () => {
  await assistantOn();
  await answer("rewrite", "Hi Priya,\n\nthe onboarding screens won't be ready by Friday. Could we move the review to Monday? I will send the updated empty states on Thursday evening.");
  await openFolder("Work", "Inbox");
  await openMessage("Q2 launch plan");
  await app.keys("r");
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
  await app.type("hi priya, the onboarding screens wont be ready by friday, can we move the review to monday? ill send the updated empty states thursday evening");
  await app.click('[data-compose="help"]');
  await mail.menuItem(app, "Fix spelling and grammar");
  await app.waitText(".writing-help .ai-text", /Thursday evening\./, 20000);
  await app.waitUntil(async () => (await app.$$(".writing-help .ai-caret")).length === 0, 10000, "the stream ends");
  await shot("writing-help");
  await app.click('.compose-foot [aria-label="Discard"]');
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
});

scene("rules", async () => {
  await app.click(".sidebar-head .icon-btn");
  await app.waitFor(".settings-nav");
  await clickText(".settings-nav-item", "Rules");
  await clickText(".set-actions .btn span", "Add a rule");
  await app.waitFor(".rule-editor");
  const set = (el, v) =>
    app.browser.execute(
      (e, value) => {
        const proto = e.tagName === "SELECT" ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, "value").set.call(e, value);
        e.dispatchEvent(new Event(e.tagName === "SELECT" ? "change" : "input", { bubbles: true }));
      },
      el,
      v,
    );
  const accounts = await app.invoke("list_accounts");
  await set(await app.$(".rule-editor .rule-grid .input"), "Invoices to Archive");
  await set(await app.$(".rule-editor .rule-grid .select"), String(accounts.find((a) => a.email === WORK).id));
  let [cond, act] = await app.$$(".rule-editor .rule-block");
  await set(await cond.$(".rule-line .input"), "accounts@example.com");
  await clickText(".rule-editor .rule-block .btn span", "Add a condition");
  [cond, act] = await app.$$(".rule-editor .rule-block");
  const second = (await cond.$$(".rule-line"))[1];
  await set(await second.$('select[aria-label="Field"]'), "subject");
  await set(await second.$(".input"), "Invoice");
  await app.waitUntil(async () => (await act.$$('select[aria-label="Folder"] option')).length > 2, 10000, "the folders");
  await set(await act.$('select[aria-label="Folder"]'), "Archive");
  await clickText(".rule-editor .rule-block .btn span", "Add an action");
  [cond, act] = await app.$$(".rule-editor .rule-block");
  await set((await act.$$('select[aria-label="Action"]'))[1], "label");
  await set(await (await act.$$(".rule-line"))[1].$(".input"), "$Invoice");
  await clickText(".rule-tools .btn span", "Which mails match?");
  await app.waitFor(".rule-preview");
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("rules");
  await app.click('.rule-editor [aria-label="Close"]');
  await app.click('.settings-head [aria-label="Close"]');
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
});

scene("contact-card", async () => {
  const card = [
    "BEGIN:VCARD",
    "VERSION:3.0",
    "FN:Daniel Okafor",
    "N:Okafor;Daniel;;;",
    "EMAIL;TYPE=WORK:daniel@example.com",
    "TEL;TYPE=WORK:+44 20 7946 0321",
    "ORG:Northwind Studio;Product",
    `PHOTO;ENCODING=b;TYPE=PNG:${portrait(128, [[206, 216, 232], [160, 178, 206], [168, 120, 92], [64, 84, 112]]).toString("base64")}`,
    "END:VCARD",
    "",
  ].join("\r\n");
  await fetch(`http://127.0.0.1:${work.control}/dav-card?name=daniel`, { method: "POST", body: card });
  await app.invoke("carddav_add", { name: "Northwind", url: `http://127.0.0.1:${work.control}/dav`, username: WORK, password: "sample-password" });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
  await openFolder("Work", "Inbox");
  await openMessage("Customer interviews: notes");
  await app.waitFor(".message-card .avatar-picture img", 10000);
  await app.click(".message-card .avatar-btn");
  await app.waitText(".contact-card .contact-row-subject", /Q2 launch plan/);
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("contact-card");
  await app.click('.contact-card [aria-label="Close"]');
});

scene("attachments", async () => {
  await mail.openView(app, "Attachments");
  await app.waitText(".hub-item .hub-name", /lake-morning\.png/, 15000);
  for (const item of await app.$$(".hub-item")) {
    if ((await app.textOf(item)).includes("lake-morning.png")) await (await item.$('[title="Open the message"]')).click();
  }
  await app.waitText(".message-subject", /Photos from the offsite/);
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("attachments");
});

scene("narrow", async () => {
  await app.browser.setWindowSize(900, 720);
  await openFolder("Personal", "Inbox");
  await openMessage("Lunch on Friday?");
  await shot("narrow");
});

scene("zero", async () => {
  // Everything done in the personal inbox: the calm empty state, the other tabs with unread mail,
  // and a snoozed message that comes back.
  await app.browser.setWindowSize(1440, 900);
  await openFolder("Personal", "Inbox");
  await openMessage("Apartment viewing on Saturday");
  // Snoozed for a week of real time (the screenshots' clock is in the past).
  const id = Number(await app.browser.execute(() => document.querySelector(".row[aria-current='true']")?.dataset.id));
  await app.invoke("snooze", { ids: [id], until: Math.floor(Date.now() / 1000) + 7 * 86400 });
  await app.click(".sidebar");
  await openFolder("Personal", "Inbox");
  await mail.waitRow(app, "Apartment viewing on Saturday", false);
  await openMessage("Lunch on Friday?");
  await app.keys("e");
  await app.waitFor(".inbox-zero", 15000);
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("inbox-zero");
});

// ---------------------------------------------------------------- calendar, meetings, encryption

/** `20260312T103000Z` of a day of the screenshots' week (0 = Monday 9 March 2026) at `hh:mm` UTC. */
const when = (day, hh, mm = 0) => new Date(Date.UTC(2026, 2, 9 + day, hh, mm)).toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");

function vcal(uid, title, start, end, extra = []) {
  return ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Example//Calendar//EN", "BEGIN:VEVENT", `UID:${uid}`, "DTSTAMP:20260301T090000Z", `DTSTART:${start}`, `DTEND:${end}`, `SUMMARY:${title}`, ...extra, "END:VEVENT", "END:VCALENDAR", ""].join("\r\n");
}

const putEvent = (calendar, name, ics) => fetch(`http://127.0.0.1:${work.control}/caldav-event?calendar=${calendar}&name=${name}`, { method: "POST", body: ics });

scene("calendar", async () => {
  await app.browser.setWindowSize(1440, 900);
  const day = (d) => [when(d, 0).slice(0, 8), when(d + 1, 0).slice(0, 8)];
  const events = [
    ["work", "standup", vcal("standup@example.com", "Standup", when(0, 9, 30), when(0, 9, 45), ["RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=20"])],
    ["work", "review", vcal("review@example.com", "Design review", when(0, 13), when(0, 14), ["LOCATION:Studio 2"])],
    ["work", "lumen", vcal("lumen@example.com", "Customer interview: Lumen", when(1, 11), when(1, 12, 30), ["LOCATION:https://teams.microsoft.com/l/meetup-join/19%3ameeting_lumen"])],
    ["work", "daniel", vcal("daniel-1on1@example.com", "1:1 Daniel", when(1, 15), when(1, 15, 30))],
    ["work", "workshop", vcal("workshop@example.com", "Workshop: onboarding flow", when(2, 14), when(2, 16), ["LOCATION:Studio 1"])],
    ["home", "climbing", vcal("climbing@example.net", "Climbing", when(2, 18, 30), when(2, 20))],
    [
      "work",
      "q2",
      vcal("q2-planning@example.com", "Q2 launch planning", when(3, 10, 30), when(3, 11, 30), [
        "LOCATION:https://meet.jit.si/q2-launch-planning",
        "ORGANIZER;CN=Emma Larsen:mailto:emma@example.com",
        "ATTENDEE;CN=Daniel Okafor;PARTSTAT=ACCEPTED:mailto:daniel@example.com",
        "ATTENDEE;CN=Priya Raman;PARTSTAT=TENTATIVE:mailto:priya@example.com",
        "ATTENDEE;CN=Jonas Weber;PARTSTAT=ACCEPTED:mailto:jonas@example.com",
        "DESCRIPTION:Scope\\, dates and owners for the Q2 launch.",
      ]),
    ],
    ["home", "lunch", vcal("lunch@example.net", "Lunch with Ana", when(3, 12, 30), when(3, 13, 30), ["LOCATION:Café Lindgren"])],
    ["work", "debrief", vcal("debrief@example.com", "Interview debrief", when(3, 16), when(3, 17))],
    ["work", "demo", vcal("demo@example.com", "Sprint demo", when(4, 11), when(4, 12))],
    ["home", "dinner", vcal("dinner@example.net", "Dinner with Leo", when(4, 19), when(4, 21, 30))],
    ["home", "market", vcal("market@example.net", "Farmers market", when(5, 10), when(5, 12))],
    ["work", "priya-out", vcal("priya-out@example.com", "Priya out of office", `${day(3)[0]}`, `${day(3)[1]}`, ["TRANSP:TRANSPARENT"]).replace(/DTSTART:(\d{8})\r\n/, "DTSTART;VALUE=DATE:$1\r\n").replace(/DTEND:(\d{8})\r\n/, "DTEND;VALUE=DATE:$1\r\n")],
  ];
  for (const [cal, name, ics] of events) await putEvent(cal, `${name}.ics`, ics);
  await app.invoke("caldav_add", { name: "Example Cloud", url: `http://127.0.0.1:${work.control}/caldav/`, username: WORK, password: "sample-password" });
  const overview = await app.invoke("calendar_overview");
  for (const c of overview.calendars) {
    if (c.name === "Arbeit") await app.invoke("calendar_set", { id: c.id, patch: { name: "Work", color: "#2563eb" } });
    if (c.name === "Privat") await app.invoke("calendar_set", { id: c.id, patch: { name: "Personal", color: "#0d9488" } });
    if (c.kind === "local") await app.invoke("calendar_set", { id: c.id, patch: { hidden: true } });
  }
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
  await mail.openView(app, "Calendar");
  await clickText(".calv-views .segmented button", "Week");
  await app.waitText(".calv-ev .calv-ev-title", /Q2 launch planning/, 20000);
  for (const e of await app.$$(".calv-ev")) if ((await app.textOf(e)).includes("Q2 launch planning")) await e.click();
  await app.waitFor(".calv-detail .calv-join");
  await app.browser.execute(() => {
    document.activeElement?.blur();
    document.querySelector(".calv-scroll").scrollTop = 7.6 * 48;
  });
  await shot("calendar-week-light");
  await setTheme("dark");
  await mail.openView(app, "Calendar");
  await app.waitText(".calv-ev .calv-ev-title", /Q2 launch planning/, 20000);
  for (const e of await app.$$(".calv-ev")) if ((await app.textOf(e)).includes("Q2 launch planning")) await e.click();
  await app.waitFor(".calv-detail .calv-join");
  await app.browser.execute(() => {
    document.activeElement?.blur();
    document.querySelector(".calv-scroll").scrollTop = 7.6 * 48;
  });
  await shot("calendar-week-dark");
  await setTheme("light");
});

scene("invitation", async () => {
  const ics = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Example//Calendar//EN",
    "METHOD:REQUEST",
    "BEGIN:VEVENT",
    "UID:roadmap-review@example.com",
    "SEQUENCE:0",
    "DTSTAMP:20260312T090000Z",
    `DTSTART:${when(3, 15, 30)}`,
    `DTEND:${when(3, 16, 30)}`,
    "SUMMARY:Roadmap review",
    "LOCATION:https://teams.microsoft.com/l/meetup-join/19%3ameeting_roadmap",
    "ORGANIZER;CN=Priya Raman:mailto:priya@example.com",
    "ATTENDEE;CN=Emma Larsen;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:emma@example.com",
    "ATTENDEE;CN=Daniel Okafor;PARTSTAT=ACCEPTED:mailto:daniel@example.com",
    "ATTENDEE;CN=Jonas Weber;PARTSTAT=TENTATIVE:mailto:jonas@example.com",
    "END:VEVENT",
    "END:VCALENDAR",
    "",
  ].join("\r\n");
  const raw = [
    "From: Priya Raman <priya@example.com>",
    `To: Emma Larsen <${WORK}>`,
    "Subject: Invitation: Roadmap review",
    "Date: Thu, 12 Mar 2026 09:58:00 +0000",
    "Message-ID: <roadmap-invite@example.com>",
    "MIME-Version: 1.0",
    'Content-Type: multipart/alternative; boundary="ALT"',
    "",
    "--ALT",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Hi Emma, let's go through the roadmap before the launch planning wraps up. Priya",
    "--ALT",
    "Content-Type: text/calendar; charset=utf-8; method=REQUEST",
    "",
    ics,
    "--ALT--",
    "",
  ].join("\r\n");
  await work.deliver(raw, { date: NOW - 2640 });
  await openFolder("Work", "Inbox");
  await openMessage("Invitation: Roadmap review");
  await app.waitFor(".invite-card", 15000);
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("invitation-card");
});

scene("encrypted", async () => {
  const accounts = await app.invoke("list_accounts");
  const emma = accounts.find((a) => a.email === WORK);
  await app.invoke("pgp_generate", { accountId: emma.id });
  const armored = await (await fetch(`http://127.0.0.1:${work.control}/pgp/public`)).text();
  await app.invoke("pgp_import", { text: armored });
  // Jonas learns Emma's key from a signed message (Autocrypt) and answers encrypted.
  await app.invoke("send_message", {
    draft: { account_id: emma.id, to: ["jonas@example.org"], cc: [], bcc: [], subject: "Contract draft", text: "Here is the draft, signed.", html: null, attachments: [], in_reply_to: null, references: [], message_id: null, reply_to_id: null, forwarded: [], protect: { method: "pgp", sign: true, encrypt: false } },
    followupSecs: null,
  });
  await app.waitUntil(async () => (await work.sent()).length > 0, 20000, "sent");
  const r = await fetch(`http://127.0.0.1:${work.control}/pgp/deliver?subject=${encodeURIComponent("Re: Contract draft")}&date=${NOW - 420}`, {
    method: "POST",
    body: "Hi Emma,\n\nthanks, the draft reads well. Two small notes on section 4; I marked them in the copy you sent.\n\nJonas",
  });
  if (!r.ok) throw new Error(await r.text());
  await openFolder("Work", "Inbox");
  await openMessage("Re: Contract draft");
  await app.waitFor(".protect-badges", 15000);
  await app.browser.execute(() => document.activeElement?.blur());
  await shot("encrypted-mail");
});
