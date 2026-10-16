// Sample data for `npm run dev` in a plain browser (no Tauri backend): the layout, themes and
// flows can be looked at without an account. Never used inside the app.

import type { Account, Mailbox, MessageSummary, MessageView } from "./types";

const now = Math.floor(Date.now() / 1000);
const H = 3600;

export const demoAccounts: Account[] = [
  { id: 1, provider: "imap", name: "Arbeit", email: "maria@example.org", sender_name: "Maria Muster" },
  { id: 2, provider: "google", name: "Privat", email: "maria.muster@example.com", sender_name: "Maria" },
];

export const demoMailboxes: Mailbox[] = [
  { id: 1, account_id: 1, name: "INBOX", delimiter: "/", role: "inbox", selectable: true, unread: 2, total: 4 },
  { id: 2, account_id: 1, name: "Drafts", delimiter: "/", role: "drafts", selectable: true, unread: 0, total: 0 },
  { id: 3, account_id: 1, name: "Sent", delimiter: "/", role: "sent", selectable: true, unread: 0, total: 1 },
  { id: 4, account_id: 1, name: "Archive", delimiter: "/", role: "archive", selectable: true, unread: 0, total: 0 },
  { id: 5, account_id: 1, name: "Projekte/Website", delimiter: "/", role: null, selectable: true, unread: 0, total: 0 },
  { id: 6, account_id: 1, name: "Trash", delimiter: "/", role: "trash", selectable: true, unread: 0, total: 0 },
  { id: 7, account_id: 2, name: "INBOX", delimiter: "/", role: "inbox", selectable: true, unread: 1, total: 1 },
  { id: 8, account_id: 2, name: "[Gmail]/Spam", delimiter: "/", role: "junk", selectable: true, unread: 0, total: 0 },
];

type Demo = MessageSummary & { to: MessageView["to"]; cc: MessageView["cc"]; html: string; text: string; blocked: number };

const m = (id: number, mailbox: number, from: [string, string], subject: string, ago: number, text: string, extra: Partial<Demo> = {}): Demo => ({
  id,
  mailbox_id: mailbox,
  uid: id,
  thread_id: id,
  subject,
  from: { name: from[0], email: from[1] },
  date: now - ago,
  snippet: text.replace(/\s+/g, " ").slice(0, 160),
  seen: true,
  flagged: false,
  answered: false,
  has_attachments: false,
  to: [{ name: "Maria Muster", email: "maria@example.org" }],
  cc: [],
  html: `<div class="plain">${text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/\n/g, "<br>")}</div>`,
  text,
  blocked: 0,
  ...extra,
});

const messages: Demo[] = [
  m(1, 1, ["Jonas Berger", "jonas@example.org"], "Angebot Website-Relaunch", 0.6 * H,
    "Hallo Maria,\n\nanbei das überarbeitete Angebot für den Relaunch. Die Positionen für Hosting und Pflege habe ich getrennt aufgeführt.\n\nViele Grüße\nJonas",
    { seen: false, has_attachments: true, cc: [{ name: "Team", email: "team@example.org" }] }),
  m(2, 1, ["Newsletter Stadtwerke", "news@stadtwerke.example"], "Ihre Verbrauchsübersicht im Oktober", 5 * H,
    "Die Übersicht enthält Bilder, die von einem Server geladen würden.",
    { seen: false, blocked: 3,
      html: '<table width="100%" style="background-color:#f3f5f8;padding:16px"><tr><td><h2 style="color:#123a6b;margin:0 0 8px">Ihre Verbrauchsübersicht</h2><p>Strom: 182 kWh, Wasser: 3,4 m³.</p><p><a href="https://stadtwerke.example/konto">Zum Kundenkonto</a></p></td></tr></table>' }),
  m(3, 1, ["Lea Hoffmann", "lea@example.org"], "Re: Termin nächste Woche", 27 * H,
    "Dienstag 10 Uhr passt mir gut. Ich bringe die Entwürfe mit.\n\n> Passt dir Dienstag oder Mittwoch?",
    { answered: true, flagged: true }),
  m(4, 1, ["GitHub", "noreply@github.example"], "[MouseWerk/arcalo-mail] CI passed on main", 4 * 24 * H, "All checks have passed."),
  m(5, 3, ["Maria Muster", "maria@example.org"], "Unterlagen für Freitag", 2 * 24 * H, "Hier die Unterlagen für Freitag."),
  m(6, 7, ["Paket-Service", "info@paket.example"], "Deine Sendung kommt heute", 2 * H, "Zustellung zwischen 12 und 15 Uhr.", { seen: false }),
];

const summary = ({ to: _to, cc: _cc, html: _html, text: _text, blocked: _b, ...s }: Demo): MessageSummary => s;

export const demo = {
  messages: (mailboxId: number): MessageSummary[] => messages.filter((x) => x.mailbox_id === mailboxId).sort((a, b) => b.date - a.date).map(summary),
  message: (id: number, allowRemote: boolean): MessageView => {
    const x = messages.find((y) => y.id === id) ?? messages[0];
    return {
      summary: summary(x),
      to: x.to,
      cc: x.cc,
      message_id: `${x.id}@demo`,
      has_body: true,
      text: x.text,
      remote_allowed: allowRemote,
      body: { html: x.html, blocked_remote: allowRemote ? 0 : x.blocked },
    };
  },
  search: (q: string): MessageSummary[] => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    return messages
      .filter((x) => words.every((w) => `${x.subject} ${x.from?.name} ${x.from?.email} ${x.text}`.toLowerCase().includes(w)))
      .map(summary);
  },
  setSeen: (id: number, seen: boolean) => {
    const x = messages.find((y) => y.id === id);
    if (x) x.seen = seen;
  },
};
