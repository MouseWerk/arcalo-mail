// Sample data for `npm run dev` in a plain browser (no Tauri backend): the layout, themes and
// flows can be looked at without an account. Never used inside the app.

import type { AiItem, CalChanged, CalendarEvent, CalendarInfo, CalendarOverview, CalFields, CalUndo, EventDraft } from "./types";
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
  Category,
  InboxCounts,
  InboxQuery,
  Scheduled,
  Subscription,
  TimeCounts,
  Unsubscribed,
  UnsubscribePlan,
  WaitingSender,
  AiOutcome,
  AiRequest,
  ContactCard,
  Destination,
  HubFilter,
  HubItem,
  Rule,
  Template,
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

type Demo = MessageSummary & {
  to: MessageView["to"];
  cc: MessageView["cc"];
  html: string;
  text: string;
  blocked: number;
  hidden?: boolean;
  later?: boolean;
  snoozed?: number | null;
  followup?: number | null;
  unsub?: string;
};

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
  account_id: mailbox === 7 || mailbox === 8 ? 2 : 1,
  category: "people",
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
    { seen: false, blocked: 3, category: "newsletters", unsub: "<https://stadtwerke.example/abmelden>",
      html: '<table width="100%" style="padding:16px"><tr><td><h2 style="margin:0 0 8px">Ihre Verbrauchsübersicht</h2><p>Strom: 182 kWh, Wasser: 3,4 m³.</p><p><a href="https://stadtwerke.example/konto">Zum Kundenkonto</a></p></td></tr></table>' }),
  m(3, 1, ["Lea Hoffmann", "lea@example.org"], "Re: Termin nächste Woche", 27 * H,
    "Dienstag 10 Uhr passt mir gut. Ich bringe die Entwürfe mit.", { answered: true, flagged: true, thread_id: 3, thread_count: 2 }),
  m(10, 3, ["Maria Muster", "maria@example.org"], "Termin nächste Woche", 30 * H, "Passt dir Dienstag oder Mittwoch?", { thread_id: 3 }),
  m(4, 1, ["Build-Server", "ci@example.org"], "Alle Prüfungen bestanden", 4 * 24 * H, "Alle Prüfungen auf main sind bestanden.", { category: "notifications" }),
  m(5, 3, ["Maria Muster", "maria@example.org"], "Unterlagen für Freitag", 2 * 24 * H, "Hier die Unterlagen für Freitag."),
  m(6, 7, ["Paket-Service", "info@paket.example"], "Deine Sendung kommt heute", 2 * H, "Zustellung zwischen 12 und 15 Uhr.", { seen: false, category: "notifications" }),
];

const senderCategory = new Map<string, Category | null>();
const screened = new Map<string, "allowed" | "blocked" | null>();
const unsubscribed = new Map<string, number>();
let scheduledList: (Scheduled & { draft: ComposeDraft })[] = [];
const catOf = (x: Demo): Category => senderCategory.get(x.from?.email ?? "") ?? x.category ?? "people";

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
  swipe_left: "archive",
  swipe_right: "read",
  background_check: true,
  screener: false,
  screener_since: null,
  focus: false,
  unified_inbox: true,
  bundles: true,
  ai: { providers: [], default_provider: "", accounts: {}, auto_summary: true, priority: true, cloud_priority: false, tone: true },
  bimi: true,
  gravatar: false,
  calendar: { default_calendar: null, entry_minutes: 30, workdays: [1, 2, 3, 4, 5] },
  autocrypt: true,
  sign_by_default: null,
  arcalo_target: "page",
  updates: "auto",
  startup_animation: true,
  phishing_warning: true,
};
const listeners = new Set<(n: Notice) => void>();

const summary = ({ to: _to, cc: _cc, html: _html, text: _text, blocked: _b, hidden: _h, later: _l, snoozed: _s, followup: _f, unsub: _u, ...s }: Demo): MessageSummary => ({
  ...s,
  category: catOf({ ...s, to: [], cc: [], html: "", text: "", blocked: 0 }),
});
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
    touch: false,
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
      body: { html: x.html, full_html: null, text_html: x.html.includes("plain") ? null : plain(x.text), blocked_remote: allowed ? 0 : x.blocked, trackers: x.blocked ? 2 : 0, plain: false },
      unsubscribe: x.unsub ?? null,
      list_id: null,
      one_click: !!x.unsub,
      marks: { pinned: !!x.pinned, later: !!x.later, snoozed_until: x.snoozed ?? null, followup_due: x.followup ?? null },
      unsubscribed: unsubscribed.get(x.from?.email ?? "") ?? null,
      sender_trusted: senders.has(x.from?.email ?? ""),
      files: x.has_attachments ? [{ id: 1, message_id: x.id, part_index: 0, filename: "Angebot-Relaunch.pdf", mime_type: "application/pdf", size: 182_000, content_id: null, inline: false, section: null }] : [],
      load_error: null,
      protection: null,
      protected_files: [],
      invitation: null,
      meeting: null,
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
  inboxList: (q: InboxQuery) =>
    visible()
      .filter((x) => mailboxes.find((b) => b.id === x.mailbox_id)?.role === "inbox")
      .filter((x) => q.account == null || x.account_id === q.account)
      .filter((x) => !x.snoozed && screened.get(x.from?.email ?? "") !== "blocked"),
  inbox: (q: InboxQuery, offset: number, limit: number): Page => {
    const list = demo
      .inboxList(q)
      .filter((x) => (q.focus ? catOf(x) === "people" || x.pinned : catOf(x) === q.tab))
      .sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned) || b.date - a.date);
    return { total: list.length, offset, items: list.slice(offset, offset + limit).map(summary) };
  },
  inboxCounts: (account: number | null): InboxCounts => {
    const list = demo.inboxList({ account, tab: "people", focus: false, threads: false, bundles: false });
    const cats: Category[] = ["people", "newsletters", "notifications", "receipts", "other"];
    const count = (f: (x: Demo) => boolean) => ({ total: list.filter(f).length, unread: list.filter((x) => f(x) && !x.seen).length });
    return {
      tabs: cats.map((c) => ({ category: c, ...count((x) => catOf(x) === c) })),
      focus: { category: "people", ...count((x) => catOf(x) === "people" || !!x.pinned) },
      screener: 0,
    };
  },
  bundle: (sender: string): MessageSummary[] => visible().filter((x) => x.from?.email === sender).sort((a, b) => b.date - a.date).map(summary),
  setSenderCategory: (email: string, c: Category | null) => void senderCategory.set(email, c),
  screenerSenders: (): WaitingSender[] => [],
  screen: (email: string, v: "allowed" | "blocked" | null): Undo => {
    screened.set(email, v);
    return { outbox: [], hidden: [], accounts: [] };
  },
  blockedSenders: () => [...screened].filter(([, v]) => v === "blocked").map(([k]) => k),
  subscriptions: (): Subscription[] =>
    visible()
      .filter((x) => x.unsub && x.from)
      .map((x) => ({ sender: x.from!.email, name: x.from!.name, list_id: null, count: 1, last: x.date, method: "one_click", unsubscribed: unsubscribed.get(x.from!.email) ?? null, message_id: x.id })),
  unsubscribePlan: (messageId: number): UnsubscribePlan => {
    const sender = messages.find((x) => x.id === messageId)?.from?.email ?? "";
    return { message_id: messageId, account_id: 1, sender, method: { kind: "one_click", url: "https://news.example/unsubscribe" } };
  },
  unsubscribe: (messageId: number): Unsubscribed => {
    unsubscribed.set(messages.find((x) => x.id === messageId)?.from?.email ?? "", now);
    return { kind: "done" };
  },
  mark: (ids: number[], what: "pinned" | "later", on: boolean) => {
    for (const x of messages) if (ids.includes(x.id)) x[what] = on;
  },
  snooze: (ids: number[], until: number | null) => {
    for (const x of messages) if (ids.includes(x.id)) x.snoozed = until;
  },
  followup: (ids: number[], due: number | null) => {
    for (const x of messages) if (ids.includes(x.id)) x.followup = due;
  },
  timeList: (which: "later" | "snoozed" | "followups"): MessageSummary[] =>
    visible()
      .filter((x) => (which === "later" ? x.later : which === "snoozed" ? x.snoozed : x.followup && x.followup <= Date.now() / 1000))
      .map((x) => ({ ...summary(x), due: which === "snoozed" ? x.snoozed : which === "followups" ? x.followup : null })),
  timeCounts: (): TimeCounts => ({
    snoozed: demo.timeList("snoozed").length,
    later: demo.timeList("later").length,
    followups: demo.timeList("followups").length,
    scheduled: scheduledList.length,
  }),
  sendLater: (d: ComposeDraft, at: number): number => {
    const id = scheduledList.length + 1;
    scheduledList = [...scheduledList, { id, account_id: d.account_id, due_at: at, subject: d.subject, recipients: d.to.join(", "), followup_secs: null, draft: d }];
    return id;
  },
  scheduled: (): Scheduled[] => scheduledList.map(({ draft: _d, ...s }) => s),
  unschedule: (id: number): ComposeDraft => {
    const s = scheduledList.find((x) => x.id === id);
    scheduledList = scheduledList.filter((x) => x.id !== id);
    if (!s) throw new Error("gone");
    return s.draft;
  },
  draftOf: (id: number): ComposeDraft => {
    const v = demo.message(id);
    return {
      account_id: v.account_id,
      to: v.to.map((a) => (a.name ? `${a.name} <${a.email}>` : a.email)),
      cc: v.cc.map((a) => (a.name ? `${a.name} <${a.email}>` : a.email)),
      bcc: [],
      subject: v.summary.subject,
      text: v.text ?? "",
      html: null,
      attachments: [],
      in_reply_to: v.in_reply_to,
      references: v.references,
      message_id: v.message_id,
      reply_to_id: null,
      forwarded: v.files.map((f) => f.id),
    };
  },
  onNotice: (f: (n: Notice) => void) => {
    listeners.add(f);
    return () => listeners.delete(f);
  },
  // The AI assistant answers with a fixed text in the browser preview, „on this device“.
  destination: (): Destination => {
    const p = settings.ai.providers[0];
    return p ? { provider: p.id, name: p.name, host: "localhost", local: true } : { provider: null, name: "", host: "", local: true };
  },
  aiRun: (request: AiRequest): AiOutcome => ({
    task: request.task,
    text:
      request.task === "summary"
        ? "- Jonas schickt das Angebot für den Relaunch.\n- Offen: deine Rückmeldung bis Freitag."
        : request.task === "rewrite"
          ? request.text
          : "Hallo Jonas,\n\ndanke, ich schaue es mir bis Freitag an.\n\nViele Grüße",
    cancelled: false,
    cached: false,
    destination: demo.destination(),
    sources: [],
    items: [],
    important: null,
    reason: null,
  }),
  rules: [] as Rule[],
  templates: [] as Template[],
  contactCard: (email: string): ContactCard => {
    const list = visible().filter((x) => x.from?.email === email).map(summary);
    return { email, name: list[0]?.from?.name ?? null, org: null, phones: [], emails: [], book: null, received: list.length, sent: 0, recent: list.slice(0, 8), files: [] };
  },
  hub: (_f: HubFilter): { total: number; offset: number; items: HubItem[] } => ({ total: 0, offset: 0, items: [] }),
  eventDraft: (item: AiItem): EventDraft => {
    const start = item.date ? new Date(`${item.date}T${item.time ?? "09:00"}:00`) : new Date();
    return { title: item.title, start: start.toISOString(), end: new Date(start.getTime() + 3600e3).toISOString(), all_day: !item.time, location: item.place ?? "", description: "" };
  },
  calendar: demoCalendar(),
};

/** A calendar on this device with a few appointments around today, for the browser preview. */
function demoCalendar() {
  const cals: CalendarInfo[] = [
    { id: 1, source_id: 1, kind: "local", source_name: "Auf diesem Gerät", remote_id: "local", name: "Kalender", color: "#2563eb", writable: true, hidden: false, account_id: null },
    { id: 2, source_id: 2, kind: "ics", source_name: "Feiertage", remote_id: "ics", name: "Feiertage", color: "#0d9488", writable: false, hidden: false, account_id: null },
  ];
  const day = (offset: number, h: number, m = 0) => {
    const d = new Date();
    d.setHours(h, m, 0, 0);
    d.setDate(d.getDate() + offset);
    return d.toISOString();
  };
  let next = 10;
  const make = (cal: number, title: string, start: string, end: string, extra: Partial<CalendarEvent> = {}): CalendarEvent => {
    const id = next++;
    return { key: `${cal}|${id}|`, calendar_id: cal, object_id: id, uid: `demo-${id}`, instance: "", title, start, end, all_day: false, location: "", organizer: "", attendees: [], body: null, link: null, link_kind: null, busy: "busy", private: false, recurring: false, categories: [], editable: cal === 1 ? "yes" : "ics", write: null, reanswer: false, ...extra };
  };
  let events: CalendarEvent[] = [
    make(1, "Abstimmung Relaunch", day(0, 10), day(0, 11), { location: "Raum 2", link: "https://meet.jit.si/arcalo-relaunch", link_kind: "Jitsi" }),
    make(1, "Mittag mit Lea", day(0, 12, 30), day(0, 13, 30)),
    make(1, "Angebot durchsehen", day(1, 9), day(1, 10, 30)),
    make(2, "Brückentag", day(2, 0), day(3, 0), { all_day: true }),
  ];
  const deleted = new Map<number, CalendarEvent>();
  return {
    overview: (): CalendarOverview => ({
      sources: [
        { id: 1, kind: "local", name: "Auf diesem Gerät", url: "", username: "", account_id: null, auto_schedule: false, last_sync: null, error: null, pending: 0 },
        { id: 2, kind: "ics", name: "Feiertage", url: "https://example.org", username: "", account_id: null, auto_schedule: false, last_sync: Math.floor(Date.now() / 1000), error: null, pending: 0 },
      ],
      calendars: cals,
      default: 1,
      presets: [["iCloud", "https://caldav.icloud.com/"]],
      accounts: [],
    }),
    events: (from: string, to: string) => events.filter((e) => e.start < to && e.end > from && !cals.find((c) => c.id === e.calendar_id)?.hidden),
    create: (calendarId: number | null, f: CalFields): CalChanged => {
      const e = make(calendarId ?? 1, f.title ?? "", f.start ?? day(0, 9), f.end ?? day(0, 10), { location: f.location ?? "" });
      events = [...events, e];
      return { event: e, undo: { object_id: e.object_id, kind: "created", before: null } };
    },
    update: (key: string, f: CalFields): CalChanged => {
      const was = events.find((e) => e.key === key)!;
      const e = { ...was, ...f } as CalendarEvent;
      events = events.map((x) => (x.key === key ? e : x));
      return { event: e, undo: { object_id: e.object_id, kind: "changed", before: JSON.stringify(was) } };
    },
    remove: (key: string): CalUndo => {
      const was = events.find((e) => e.key === key)!;
      deleted.set(was.object_id, was);
      events = events.filter((e) => e.key !== key);
      return { object_id: was.object_id, kind: "deleted", before: null };
    },
    undo: (u: CalUndo): CalendarEvent | null => {
      if (u.kind === "created") {
        events = events.filter((e) => e.object_id !== u.object_id);
        return null;
      }
      const back = u.kind === "deleted" ? deleted.get(u.object_id) : u.before ? (JSON.parse(u.before) as CalendarEvent) : undefined;
      if (!back) return null;
      events = [...events.filter((e) => e.object_id !== u.object_id), back];
      return back;
    },
    set: (id: number, patch: { color?: string; hidden?: boolean; name?: string }): CalendarInfo => {
      const i = cals.findIndex((c) => c.id === id);
      cals[i] = { ...cals[i], ...patch };
      return cals[i];
    },
  };
}
