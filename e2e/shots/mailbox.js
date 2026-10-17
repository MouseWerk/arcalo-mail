// The sample mailboxes of the README screenshots: Emma Larsen, a product designer, with a work
// account and a personal one. Everyone and everything here is made up; every address is on a
// reserved example domain. Dates are relative to the fixed clock of the screenshots.

import { message } from "../lib/server.js";
import { landscape, pdf } from "../lib/png.js";

/** Thursday, 12 March 2026, 10:42 (UTC, which is London time in March). */
export const NOW = Date.UTC(2026, 2, 12, 10, 42) / 1000;

export const WORK = "emma@example.com";
export const PERSONAL = "emma.larsen@example.net";
const EMMA = `Emma Larsen <${WORK}>`;

const DANIEL = "Daniel Okafor <daniel@example.com>";
const PRIYA = "Priya Raman <priya@example.com>";
const JONAS = "Jonas Weber <jonas@example.com>";
const LEO = "Leo Fischer <leo@example.org>";
const ANA = "Ana Costa <ana@example.org>";

/** A time `days` before the fixed day at `hh:mm`. */
const at = (days, hh, mm) => new Date((NOW - (NOW % 86400) - days * 86400 + hh * 3600 + mm * 60) * 1000);

const newsletter = `<!doctype html>
<html><body style="margin:0;background:#f4f1ec;font-family:Georgia,serif;color:#2b2a28">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background:#f4f1ec">
<tr><td align="center" style="padding:28px 12px">
<table role="presentation" width="600" cellpadding="0" cellspacing="0" style="background:#ffffff;border-radius:6px">
<tr><td style="padding:28px 36px 8px 36px">
  <img src="https://images.fieldnotes.example/brand/wordmark.png" width="132" height="28" alt="Field Notes">
  <p style="font:13px/1.4 Helvetica,Arial,sans-serif;color:#8a8580;margin:18px 0 0">Issue 48 &middot; A letter about making software</p>
</td></tr>
<tr><td style="padding:8px 36px 0 36px">
  <img src="https://images.fieldnotes.example/48/hero.jpg" width="528" height="220" alt="A quiet desk by a window">
</td></tr>
<tr><td style="padding:18px 36px 4px 36px">
  <h1 style="font-weight:normal;font-size:28px;line-height:1.25;margin:0 0 12px">Designing for slow software</h1>
  <p style="font-size:17px;line-height:1.6;margin:0 0 14px">Most of what we build is fast on the machine we build it on. This week: what changes when you design for the train, the old laptop and the full disk, and why the best loading state is the one nobody sees.</p>
  <p style="font-size:17px;line-height:1.6;margin:0 0 20px">Plus three tools we started using this month, and a reader question about offline first.</p>
  <p style="margin:0 0 28px"><a href="https://fieldnotes.example/48" style="display:inline-block;background:#2b2a28;color:#ffffff;text-decoration:none;font:15px Helvetica,Arial,sans-serif;padding:11px 20px;border-radius:4px">Read the issue</a></p>
</td></tr>
<tr><td style="padding:0 36px 28px 36px;border-top:1px solid #ece8e2">
  <h2 style="font-weight:normal;font-size:20px;margin:22px 0 8px">Three tools this month</h2>
  <p style="font-size:16px;line-height:1.6;margin:0">A sketchbook app that works without an account, a plain-text task list, and a font for long technical documents.</p>
  <img src="https://track.fieldnotes.example/open/48/emma" width="1" height="1" alt="">
</td></tr>
</table>
<p style="font:12px/1.5 Helvetica,Arial,sans-serif;color:#8a8580;margin:16px 0 0">You get this because you subscribed at fieldnotes.example. <a href="https://fieldnotes.example/unsubscribe" style="color:#8a8580">Unsubscribe</a></p>
</td></tr></table>
</body></html>`;

const newsletterText = `Field Notes, issue 48: Designing for slow software

Most of what we build is fast on the machine we build it on. This week: what changes when you design for the train, the old laptop and the full disk.

Read the issue: https://fieldnotes.example/48`;

const shipping = `<!doctype html><html><body style="font-family:Helvetica,Arial,sans-serif;color:#1d2433;background:#ffffff">
<div style="max-width:560px;margin:0 auto;padding:24px">
<p style="font-size:13px;color:#6b7280;margin:0 0 18px">Order 4417-2209</p>
<h1 style="font-size:22px;margin:0 0 10px">Your order has shipped</h1>
<p style="font-size:15px;line-height:1.55">Good news: your desk lamp is on its way and should arrive on Monday, 9 March.</p>
<p style="font-size:15px;line-height:1.55">Track it any time with the code <b>PX 2291 0477 31</b>.</p>
<img src="https://cdn.parcels.example/map/4417-2209.png" width="512" height="160" alt="Delivery route">
</div></body></html>`;

const invite = [
  "BEGIN:VCALENDAR",
  "VERSION:2.0",
  "PRODID:-//Example//Calendar//EN",
  "METHOD:REQUEST",
  "BEGIN:VEVENT",
  "UID:design-review-0313@example.com",
  "DTSTART:20260313T150000Z",
  "DTEND:20260313T160000Z",
  "SUMMARY:Design review: onboarding flow",
  "LOCATION:Room 3 / video link in the description",
  `ORGANIZER;CN=Priya Raman:mailto:priya@example.com`,
  `ATTENDEE;CN=Emma Larsen;RSVP=TRUE:mailto:${WORK}`,
  "END:VEVENT",
  "END:VCALENDAR",
  "",
].join("\r\n");

/** The list headers of a newsletter with one-click unsubscribe (RFC 8058) at `url`. */
const oneClick = (id, url) => [`List-Id: <${id}>`, `List-Unsubscribe: <${url}>`, "List-Unsubscribe-Post: List-Unsubscribe=One-Click"];

/** Messages of the work account: [folder, message, flags]. `web`: an address on the test server
 *  (the one newsletter the screenshots unsubscribe from). */
export function workMail(web = (p) => `https://example.com${p}`) {
  const out = [];
  const add = (mailbox, m, flags = {}) => out.push({ mailbox, raw: message(m), date: m.date, flags });

  // A conversation: Daniel's plan, Emma's answer (in Sent), Priya's reply.
  add("INBOX", {
    from: DANIEL,
    to: EMMA,
    cc: PRIYA,
    subject: "Q2 launch plan",
    id: "q2-plan@example.com",
    date: at(0, 8, 54),
    body: `Hi Emma, hi Priya,

here is the first draft of the Q2 launch plan. The short version:

1. Beta for the onboarding flow from 1 April, with the twenty teams from the interviews.
2. Public launch on 6 May, together with the pricing page.
3. Help center articles and the changelog the week before.

Could you check whether the design milestones still fit? The review on Friday would be the last point to move things.

Thanks,
Daniel`,
  }, { seen: true });
  add("Sent", {
    from: EMMA,
    to: DANIEL,
    cc: PRIYA,
    subject: "Re: Q2 launch plan",
    id: "q2-plan-emma@example.com",
    inReplyTo: "q2-plan@example.com",
    date: at(0, 9, 37),
    body: `Hi Daniel,

thanks, this reads well. The milestones fit if we freeze the onboarding screens by 20 March. I would like one more round with two of the beta teams before that.

Emma

On Thursday, 12 March 2026 at 08:54, Daniel Okafor wrote:
> here is the first draft of the Q2 launch plan.`,
  }, { seen: true });
  add("INBOX", {
    from: PRIYA,
    to: `${EMMA}, ${DANIEL}`,
    subject: "Re: Q2 launch plan",
    id: "q2-plan-priya@example.com",
    inReplyTo: "q2-plan-emma@example.com",
    date: at(0, 10, 31),
    body: `Sounds good to me. I booked two sessions with the beta teams for Tuesday and Wednesday next week, 30 minutes each, and put the invites in your calendars.

For the review on Friday I will bring the updated empty states and the copy for the welcome screen.

Priya`,
  });

  const fieldNotes = oneClick("letters.fieldnotes.example", "https://fieldnotes.example/unsubscribe/one-click");
  add("INBOX", { from: "Field Notes <letters@fieldnotes.example>", to: EMMA, subject: "Field Notes #48: Designing for slow software", date: at(0, 9, 15), body: newsletterText, html: newsletter, headers: fieldNotes });
  add("INBOX", { from: "Field Notes <letters@fieldnotes.example>", to: EMMA, subject: "Field Notes #47: The quiet interface", date: at(7, 9, 15), body: "Field Notes, issue 47: The quiet interface.\n\nWhy the calmest screens are the ones that leave things out.", headers: fieldNotes }, { seen: true });
  add("INBOX", {
    from: "Design Weekly <hello@designweekly.example>",
    to: EMMA,
    subject: "Five type pairings for product teams",
    date: at(2, 6, 30),
    body: "This week: five type pairings that hold up in dense interfaces, and a reader's question about icon sizes.",
    headers: oneClick("weekly.designweekly.example", web("/unsubscribe?list=design-weekly")),
  });
  add("INBOX", {
    from: "Product Digest <digest@productdigest.example>",
    to: EMMA,
    subject: "The week in product: pricing pages",
    date: at(3, 7, 0),
    body: "Pricing pages that explain themselves, and what changed in onboarding this quarter.",
    headers: ["List-Id: <digest.productdigest.example>", "List-Unsubscribe: <mailto:leave@productdigest.example?subject=unsubscribe>"],
  }, { seen: true });
  add("INBOX", { from: "Build server <noreply@ci.example.com>", to: EMMA, subject: "Deploy finished: onboarding-beta", date: at(0, 7, 12), body: "onboarding-beta was deployed to the staging environment in 4 minutes." });
  add("INBOX", { from: "Calendar <noreply@calendar.example.com>", to: EMMA, subject: "Reminder: Design review tomorrow at 15:00", date: at(0, 6, 0), body: "Design review: onboarding flow, Friday 13 March at 15:00." }, { seen: true });
  add("INBOX", {
    from: JONAS,
    to: EMMA,
    subject: "Photos from the offsite",
    date: at(0, 8, 47),
    body: `Morning Emma,

two of the photos from the lake last Friday, the others are in the shared album. The one with the boats would make a good header for the team page.

Jonas`,
    attachments: [
      { name: "lake-morning.png", type: "image/png", data: landscape(640, 400) },
      { name: "boats.png", type: "image/png", data: landscape(640, 400, [[198, 214, 230], [246, 226, 200], [120, 136, 104], [86, 124, 150]]) },
    ],
  });
  add("INBOX", {
    from: PRIYA,
    to: EMMA,
    subject: "Invitation: Design review, Fri 13 Mar 15:00",
    date: at(1, 16, 20),
    body: `Design review: onboarding flow
Friday, 13 March 2026, 15:00 to 16:00
Room 3, or join with the video link in the calendar entry.

Agenda: empty states, welcome copy, the first-run checklist.`,
    attachments: [{ name: "invite.ics", type: "text/calendar; method=REQUEST", data: invite }],
  }, { seen: true });
  add("INBOX", {
    from: "Studio Accounts <accounts@example.com>",
    to: EMMA,
    subject: "Invoice INV-2026-031 for your software subscriptions",
    date: at(1, 11, 5),
    body: `Hello Emma,

attached is the invoice for the team's software subscriptions in March. No action is needed, the amount is booked on the design budget.

Studio Accounts`,
    attachments: [{ name: "INV-2026-031.pdf", type: "application/pdf", data: pdf("Invoice INV-2026-031", 48_000) }],
  }, { seen: true });
  add("INBOX", {
    from: DANIEL,
    to: EMMA,
    subject: "Customer interviews: notes",
    date: at(1, 9, 30),
    body: `The notes from all twelve interviews are in the research folder now. The biggest theme by far: people want to start without creating a project first.

I marked the quotes that I think belong in the review.`,
  }, { seen: true, flagged: true });
  add("INBOX", { from: LEO, to: EMMA, subject: "Draft for the case study", date: at(2, 14, 12), body: "Hi Emma,\n\nthe draft for the case study is ready for a first look. I kept it short; the screenshots are placeholders for now.\n\nLeo" }, { seen: true });
  add("INBOX", { from: ANA, to: EMMA, subject: "Accessibility workshop", id: "workshop@example.org", date: at(3, 17, 2), body: "Hi Emma,\n\nare you still in for the accessibility workshop next week? I would take the part about keyboard use if you take contrast and motion.\n\nAna" }, { seen: true });
  add("INBOX", { from: "Parcel updates <updates@parcels.example>", to: EMMA, subject: "Your order has shipped", date: at(6, 7, 40), body: "Your order 4417-2209 has shipped and should arrive on Monday, 9 March.", html: shipping }, { seen: true });
  add("INBOX", { from: JONAS, to: EMMA, subject: "Desk booking for April", date: at(7, 12, 18), body: "The desk booking for April is open. I took the two by the window for us on Tuesdays and Thursdays." }, { seen: true });
  add("INBOX", { from: DANIEL, to: EMMA, subject: "Hiring: product designer role", date: at(12, 10, 0), body: "Here is the job description for the new product designer role. Could you add the part about the portfolio review?" }, { seen: true, answered: true });
  add("INBOX", { from: PRIYA, to: EMMA, subject: "Brand guidelines v3", date: at(14, 15, 45), body: "Version 3 of the brand guidelines is final. The biggest change is the new type scale for small screens." }, { seen: true });
  add("INBOX", { from: LEO, to: EMMA, subject: "Team retro notes", date: at(20, 16, 30), body: "Notes from the retro: fewer meetings on Mondays, a shared calendar for releases, and pairing on reviews." }, { seen: true });

  add("Sent", { from: EMMA, to: ANA, subject: "Re: Accessibility workshop", inReplyTo: "workshop@example.org", date: at(3, 18, 10), body: "Yes, I am in. Contrast and motion are mine." }, { seen: true });
  add("Archive", { from: DANIEL, to: EMMA, subject: "Roadmap for the second half", date: at(40, 9, 0), body: "The roadmap for the second half of the year, as discussed." }, { seen: true });
  return out;
}

/** Messages of the personal account. */
export function personalMail() {
  const me = `Emma Larsen <${PERSONAL}>`;
  const out = [];
  const add = (mailbox, m, flags = {}) => out.push({ mailbox, raw: message(m), date: m.date, flags });
  add("INBOX", { from: "Sofia Marin <sofia.marin@example.net>", to: me, subject: "Lunch on Friday?", date: at(0, 7, 55), body: "Lunch on Friday? The new place by the river has a terrace now." });
  add("INBOX", { from: "Marta Silva <marta.silva@example.net>", to: me, subject: "Apartment viewing on Saturday", date: at(1, 19, 2), body: "The viewing is on Saturday at 11. The flat is on the third floor; ring twice." }, { seen: true });
  add("INBOX", {
    from: "City Library <events@library.example>",
    to: me,
    subject: "Book club: the April pick",
    date: at(4, 8, 0),
    body: "The book club meets on 2 April. This month we read a novel about a lighthouse keeper.",
    headers: ["List-Id: <events.library.example>", "List-Unsubscribe: <mailto:events-leave@library.example>"],
  });
  add("INBOX", { from: "Bank <no-reply@bank.example>", to: me, subject: "New sign-in to your account", date: at(0, 6, 40), body: "We noticed a new sign-in to your online banking from a browser on Linux." });
  add("INBOX", { from: "Cinema <tickets@cinema.example>", to: me, subject: "Your order 2231: two tickets for Saturday", date: at(2, 20, 15), body: "Order 2231: two tickets for Saturday, 14 March, 20:00, hall 3." }, { seen: true });
  return out;
}

/** Delivers the messages to a test server with their dates. */
export async function fill(server, list) {
  for (const m of list) await server.deliver(m.raw, { mailbox: m.mailbox, date: Math.floor(m.date.getTime() / 1000), ...m.flags });
}
