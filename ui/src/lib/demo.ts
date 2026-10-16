// Sample data for `npm run dev` in a plain browser (no Tauri backend): the layout, themes and
// flows can be looked at without an account. Never used inside the app.

import type {
  Account,
  AccountDraft,
  AccountInfo,
  AppInfo,
  CheckResult,
  ComposeDraft,
  Contact,
  Discovered,
  Mailbox,
  MessageSummary,
  MicrosoftSignIn,
  MessageView,
  Notice,
  Page,
  Queued,
  Settings,
  Undo,
} from "./types";

const now = Math.floor(Date.now() / 1000);
const H = 3600;

const transport = (email: string): Account["transport"] => ({
  kind: "imap",
  imap: { host: `imap.${email.split("@")[1]}`, port: 993, security: "tls" },
  smtp: { host: `smtp.${email.split("@")[1]}`, port: 465, security: "tls" },
  username: email,
  auth: { kind: "password" },
});

let accounts: Account[] = [
  { id: 1, provider: "imap", name: "Arbeit", email: "maria@example.org", sender_name: "Maria Muster", transport: transport("maria@example.org"), signature: "Maria Muster" },
  { id: 2, provider: "imap", name: "Privat", email: "maria.muster@example.com", sender_name: "Maria", transport: transport("maria.muster@example.com"), signature: "" },
];

let mailboxes: Mailbox[] = [
  { id: 1, account_id: 1, name: "INBOX", delimiter: "/", role: "inbox", selectable: true, unread: 0, total: 0 },
  { id: 2, account_id: 1, name: "Drafts", delimiter: "/", role: "drafts", selectable: true, unread: 0, total: 0 },
  { id: 3, account_id: 1, name: "Sent", delimiter: "/", role: "sent", selectable: true, unread: 0, total: 0 },
  { id: 4, account_id: 1, name: "Archive", delimiter: "/", role: "archive", selectable: true, unread: 0, total: 0 },
  { id: 5, account_id: 1, name: "Projekte", delimiter: "/", role: null, selectable: true, unread: 0, total: 0 },
  { id: 9, account_id: 1, name: "Projekte/Website", delimiter: "/", role: null, selectable: true, unread: 0, total: 0 },
  { id: 6, account_id: 1, name: "Trash", delimiter: "/", role: "trash", selectable: true, unread: 0, total: 0 },
  { id: 7, account_id: 2, name: "INBOX", delimiter: "/", role: "inbox", selectable: true, unread: 0, total: 0 },
  { id: 8, account_id: 2, name: "Junk", delimiter: "/", role: "junk", selectable: true, unread: 0, total: 0 },
];

type Demo = MessageSummary & { to: MessageView["to"]; cc: MessageView["cc"]; html: string; text: string; blocked: number; hidden?: boolean };

const plain = (text: string) => `<div class="plain">${text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/\n/g, "<br>")}</div>`;

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
  thread_count: 1,
  to: [{ name: "Maria Muster", email: "maria@example.org" }],
  cc: [],
  html: plain(text),
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
      html: '<table width="100%" style="padding:16px"><tr><td><h2 style="margin:0 0 8px">Ihre Verbrauchsübersicht</h2><p>Strom: 182 kWh, Wasser: 3,4 m³.</p><p><a href="https://stadtwerke.example/konto">Zum Kundenkonto</a></p></td></tr></table>' }),
  m(3, 1, ["Lea Hoffmann", "lea@example.org"], "Re: Termin nächste Woche", 27 * H,
    "Dienstag 10 Uhr passt mir gut. Ich bringe die Entwürfe mit.", { answered: true, flagged: true, thread_id: 3, thread_count: 2 }),
  m(10, 3, ["Maria Muster", "maria@example.org"], "Termin nächste Woche", 30 * H, "Passt dir Dienstag oder Mittwoch?", { thread_id: 3 }),
  m(4, 1, ["Build-Server", "ci@example.org"], "Alle Prüfungen bestanden", 4 * 24 * H, "Alle Prüfungen auf main sind bestanden."),
  m(5, 3, ["Maria Muster", "maria@example.org"], "Unterlagen für Freitag", 2 * 24 * H, "Hier die Unterlagen für Freitag."),
  m(6, 7, ["Paket-Service", "info@paket.example"], "Deine Sendung kommt heute", 2 * H, "Zustellung zwischen 12 und 15 Uhr.", { seen: false }),
];

const remote = new Set<number>();
const senders = new Set<string>();
let settings: Settings = {
  version: 2,
  language: "system",
  theme: "system",
  remote_content: "ask",
  threads: true,
  mark_read_after_secs: 0,
  notifications: true,
  undo_send_secs: 5,
  compose_rich_text: true,
  show_snippets: true,
  prefetch: "recent",
  subject_threads: true,
  network: {
    profiles: [{ id: "standard", name: "Standard", mode: "system", http_proxy: "", socks_proxy: "", no_proxy: "localhost, 127.0.0.1, ::1", proxy_user: "" }],
    routes: {},
  },
};
const listeners = new Set<(n: Notice) => void>();

const summary = ({ to: _to, cc: _cc, html: _html, text: _text, blocked: _b, hidden: _h, ...s }: Demo): MessageSummary => s;
const visible = () => messages.filter((x) => !x.hidden);

function counts(): Mailbox[] {
  return mailboxes.map((b) => {
    const list = visible().filter((x) => x.mailbox_id === b.id);
    return { ...b, total: list.length, unread: list.filter((x) => !x.seen).length };
  });
}

export const demo = {
  appInfo: (): AppInfo => ({
    version: "0.1.0",
    platform: "browser",
    schema: 3,
    locale: navigator.language,
    secret_store: { kind: "system" },
    clock: null,
    microsoft_sign_in: true,
  }),
  microsoftSignIn: (email: string): MicrosoftSignIn => ({
    session: "demo",
    email: email || "maria@outlook.example",
    check: { imap: null, smtp: null, error: null, folders: 6 },
    problem: null,
    error: null,
  }),
  addMicrosoftAccount: (_session: string, name: string): Account =>
    demo.addAccount({
      provider: "microsoft",
      name: "",
      email: "maria@outlook.example",
      sender_name: name,
      transport: { ...(transport("maria@outlook.example") as Extract<Account["transport"], { kind: "imap" }>), auth: { kind: "o_auth2", provider: "microsoft" } },
    }),
  settings: (): Settings => settings,
  saveSettings: (s: Settings) => {
    settings = s;
  },
  discover: (email: string): Discovered => {
    const domain = email.split("@")[1] ?? "example.org";
    return {
      imap: { host: `imap.${domain}`, port: 993, security: "tls" },
      smtp: { host: `smtp.${domain}`, port: 465, security: "tls" },
      username: email,
      source: "autoconfig",
      provider: null,
      hint: null,
      jmap: null,
    };
  },
  check: (password: string): CheckResult =>
    password === "falsch" ? { imap: { kind: "auth", detail: "Invalid credentials" }, smtp: null, error: null, folders: 0 } : { imap: null, smtp: null, error: null, folders: 6 },
  addAccount: (d: AccountDraft): Account => {
    const a = { ...d, id: Math.max(0, ...accounts.map((x) => x.id)) + 1, signature: "" };
    accounts = [...accounts, a];
    return a;
  },
  updateAccount: (id: number, name: string, sender: string, signature: string) => {
    accounts = accounts.map((a) => (a.id === id ? { ...a, name, sender_name: sender, signature } : a));
  },
  removeAccount: (id: number) => {
    accounts = accounts.filter((a) => a.id !== id);
  },
  accounts: (): AccountInfo[] => accounts.map((a) => ({ ...a, status: { kind: "status", account_id: a.id, status: { state: "ready" }, problem: null, error: null }, pending: 0 })),
  mailboxes: (accountId: number): Mailbox[] => counts().filter((b) => b.account_id === accountId),
  messages: (mailboxId: number, offset: number, limit: number, threads: boolean): Page => {
    let list = visible().filter((x) => x.mailbox_id === mailboxId).sort((a, b) => b.date - a.date);
    if (!threads) list = list.map((x) => ({ ...x, thread_count: 1 }));
    return { total: list.length, offset, items: list.slice(offset, offset + limit).map(summary) };
  },
  thread: (threadId: number): MessageSummary[] =>
    visible().filter((x) => x.thread_id === threadId).sort((a, b) => a.date - b.date).map(summary),
  message: (id: number): MessageView => {
    const x = messages.find((y) => y.id === id) ?? messages[0];
    const allowed = remote.has(id) || senders.has(x.from?.email ?? "");
    return {
      summary: summary(x),
      account_id: mailboxes.find((b) => b.id === x.mailbox_id)?.account_id ?? 1,
      to: x.to,
      cc: x.cc,
      reply_to: [],
      message_id: `${x.id}@demo`,
      in_reply_to: null,
      references: [],
      size: x.text.length,
      draft: false,
      has_body: true,
      text: x.text,
      remote_allowed: allowed,
      body: { html: x.html, full_html: null, text_html: x.html.includes("plain") ? null : plain(x.text), blocked_remote: allowed ? 0 : x.blocked, plain: false },
      sender_trusted: senders.has(x.from?.email ?? ""),
      files: x.has_attachments ? [{ id: 1, message_id: x.id, part_index: 0, filename: "Angebot-Relaunch.pdf", mime_type: "application/pdf", size: 182_000, content_id: null, inline: false, section: null }] : [],
      load_error: null,
    };
  },
  allowRemote: (id: number) => void remote.add(id),
  allowSender: (e: string) => void senders.add(e),
  forbidSender: (e: string) => void senders.delete(e),
  allowedSenders: () => [...senders],
  setFlags: (ids: number[], seen: boolean | null, flagged: boolean | null) => {
    for (const x of messages)
      if (ids.includes(x.id)) {
        if (seen != null) x.seen = seen;
        if (flagged != null) x.flagged = flagged;
      }
  },
  hide: (ids: number[]): Undo => {
    for (const x of messages) if (ids.includes(x.id)) x.hidden = true;
    return { outbox: [], hidden: ids, accounts: [] };
  },
  unhide: (u: Undo) => {
    for (const x of messages) if (u.hidden.includes(x.id)) x.hidden = false;
    return true;
  },
  createFolder: (accountId: number, parent: number | null, name: string) => {
    const p = mailboxes.find((b) => b.id === parent);
    const id = Math.max(...mailboxes.map((b) => b.id)) + 1;
    mailboxes = [...mailboxes, { id, account_id: accountId, name: p ? `${p.name}/${name}` : name, delimiter: "/", role: null, selectable: true, unread: 0, total: 0 }];
    return id;
  },
  renameFolder: (id: number, name: string) => {
    mailboxes = mailboxes.map((b) => (b.id === id ? { ...b, name: b.name.includes("/") ? `${b.name.slice(0, b.name.lastIndexOf("/"))}/${name}` : name } : b));
  },
  deleteFolder: (id: number) => {
    mailboxes = mailboxes.filter((b) => b.id !== id);
  },
  search: (q: string): MessageSummary[] => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    return visible()
      .filter((x) => words.every((w) => `${x.subject} ${x.from?.name} ${x.from?.email} ${x.text}`.toLowerCase().includes(w)))
      .map(summary);
  },
  suggest: (prefix: string): Contact[] => {
    const p = prefix.toLowerCase();
    const all: Contact[] = [
      { email: "jonas@example.org", name: "Jonas Berger" },
      { email: "lea@example.org", name: "Lea Hoffmann" },
      { email: "team@example.org", name: "Team" },
    ];
    return all.filter((c) => c.email.startsWith(p) || (c.name ?? "").toLowerCase().split(" ").some((w) => w.startsWith(p)));
  },
  send: (d: ComposeDraft): Queued => {
    setTimeout(() => listeners.forEach((f) => f({ kind: "sent", account_id: d.account_id, outbox_id: 1 })), settings.undo_send_secs * 1000);
    return { outbox_id: 1, message_id: "demo@example.org", delay: settings.undo_send_secs };
  },
  preview: () => ({ mime_type: "image/png", data_b64: "" }),
  onNotice: (f: (n: Notice) => void) => {
    listeners.add(f);
    return () => listeners.delete(f);
  },
};
