// The shapes the commands return and take (src-tauri/src/commands.rs, mail_core's types).

export type Provider = "imap" | "microsoft" | "google" | "jmap";
export type Role = "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "all" | "flagged" | "important";
export type Security = "tls" | "start_tls" | "plain";

export type Address = { name: string | null; email: string };

export type Server = { host: string; port: number; security: Security; trusted_cert?: string | null };

export type Auth = { kind: "password" } | { kind: "o_auth2"; provider: "microsoft" | "google" } | { kind: "token" };

export type Transport =
  | { kind: "imap"; imap: Server; smtp: Server; username: string; auth: Auth }
  | { kind: "graph" }
  | { kind: "jmap"; session_url: string; username: string; auth: Auth }
  | { kind: "pop3"; pop3: Server; smtp: Server; username: string; auth: Auth; leave_on_server: boolean; delete_after_days: number | null };

export type ImapTransport = Extract<Transport, { kind: "imap" }>;
export type Pop3Transport = Extract<Transport, { kind: "pop3" }>;

export type Account = {
  id: number;
  provider: Provider;
  name: string;
  email: string;
  sender_name: string;
  transport: Transport;
  signature: string;
};

export type AccountDraft = Omit<Account, "id" | "signature">;

/** What went wrong on the way to a mailbox (mail_core::network::Problem). */
export type Problem =
  | { kind: "dns"; host: string }
  | { kind: "refused"; host: string; port: number }
  | { kind: "timeout"; host: string; port: number }
  | { kind: "offline" }
  | { kind: "tls"; host: string; detail: string }
  | { kind: "certificate"; host: string; fingerprint: string; detail: string }
  | { kind: "auth"; detail: string }
  | { kind: "app_password"; detail: string }
  | { kind: "protocol"; detail: string }
  | { kind: "admin_consent"; detail: string }
  | { kind: "smtp_auth_disabled"; detail: string }
  | { kind: "proxy"; proxy: string; detail: string }
  | { kind: "busy"; detail: string };

export type AccountStatus =
  | { state: "connecting" }
  | { state: "syncing" }
  | { state: "ready" }
  | { state: "offline" }
  | { state: "retrying"; in_secs: number }
  | { state: "sign_in" }
  | { state: "stopped" };

export type StatusNotice = { kind: "status"; account_id: number; status: AccountStatus; problem: Problem | null; error: string | null };

/** What an account's worker tells (the `mail-notice` event). */
export type Notice =
  | StatusNotice
  | { kind: "changed"; account_id: number; mailboxes: number[] }
  | { kind: "new_mail"; account_id: number; messages: number[] }
  | { kind: "outbox"; account_id: number }
  | { kind: "sent"; account_id: number; outbox_id: number };

export type AccountInfo = Account & { status: StatusNotice | null; pending: number };

export type Discovered = {
  imap: Server;
  smtp: Server;
  username: string;
  source: "preset" | "autoconfig" | "ispdb" | "srv" | "mx" | "guess";
  provider: string | null;
  hint: "app_password" | "enable_imap" | "mail_password" | "microsoft_sign_in" | "google_app_password" | null;
  /** The JMAP session resource, when the provider offers JMAP. */
  jmap?: string | null;
};

/** The result of „Mit Microsoft anmelden“ (commands::MicrosoftSignIn). */
export type MicrosoftSignIn = {
  session: string | null;
  email: string | null;
  check: CheckResult | null;
  problem: Problem | null;
  error: string | null;
};

export type CheckResult = { imap: Problem | null; smtp: Problem | null; error: string | null; folders: number };

export type Mailbox = {
  id: number;
  account_id: number;
  name: string;
  delimiter: string | null;
  role: Role | null;
  selectable: boolean;
  unread: number;
  total: number;
  /** Why the server would not open the folder at the last try (it is tried again later). */
  sync_error?: string | null;
};

export type MessageSummary = {
  id: number;
  mailbox_id: number;
  uid: number;
  thread_id: number;
  subject: string;
  from: Address | null;
  /** Unix seconds. */
  date: number;
  snippet: string;
  seen: boolean;
  flagged: boolean;
  answered: boolean;
  has_attachments: boolean;
  /** Messages of the conversation in this folder (1 outside the conversation view). */
  thread_count: number;
  account_id: number;
  /** The inbox tab of the message (the sorted inbox only). */
  category?: Category | null;
  pinned?: boolean;
  /** A bundle of the newsletters tab: how many messages of the sender it holds (0: a plain row). */
  bundle?: number;
  /** The time tools' lists: when it comes back, is due, or was put on the list. */
  due?: number | null;
  /** „Wichtig“ (local heuristics or the AI model). */
  important?: boolean;
};

/** The tabs of the sorted inbox (mail_core::triage::Category). */
export type Category = "people" | "newsletters" | "notifications" | "receipts" | "other";

export type InboxQuery = { account: number | null; tab: Category; focus: boolean; threads: boolean; bundles: boolean };

export type TabCount = { category: Category; total: number; unread: number };

export type InboxCounts = { tabs: TabCount[]; focus: TabCount; screener: number };

/** A sender waiting in „Neue Absender“, with its messages (newest first). */
export type WaitingSender = { email: string; name: string | null; messages: MessageSummary[] };

export type Subscription = {
  sender: string;
  name: string | null;
  list_id: string | null;
  count: number;
  last: number;
  method: "one_click" | "mailto" | "link" | null;
  unsubscribed: number | null;
  message_id: number;
};

/** What „Abbestellen“ did. */
export type Unsubscribed = { kind: "done" } | { kind: "sent"; to: string } | { kind: "link"; url: string };

/** A way out of a list, as the backend read it from one message's `List-Unsubscribe`. */
export type UnsubscribeMethod = { kind: "one_click"; url: string } | { kind: "mailto"; to: string; subject: string; body: string } | { kind: "link"; url: string };

/** What „Abbestellen“ on a message would do; done only as confirmed. */
export type UnsubscribePlan = { message_id: number; account_id: number; sender: string; method: UnsubscribeMethod };

/** A message waiting to be sent later. */
export type Scheduled = { id: number; account_id: number; due_at: number; subject: string; recipients: string; followup_secs: number | null };

export type TimeCounts = { snoozed: number; later: number; followups: number; scheduled: number };

/** A reminder that came due without an answer. */
export type Reminder = { account_id: number; subject: string };

export type Marks = { pinned: boolean; later: boolean; snoozed_until: number | null; followup_due: number | null };

export type Page = { total: number; offset: number; items: MessageSummary[] };

export type Attachment = {
  id: number;
  message_id: number;
  part_index: number;
  filename: string | null;
  mime_type: string;
  size: number;
  content_id: string | null;
  inline: boolean;
  section: string | null;
};

export type MessageView = {
  summary: MessageSummary;
  account_id: number;
  to: Address[];
  cc: Address[];
  reply_to: Address[];
  message_id: string | null;
  in_reply_to: string | null;
  references: string[];
  size: number;
  draft: boolean;
  has_body: boolean;
  text: string | null;
  remote_allowed: boolean;
  /** Sanitized in the backend (mail_core::actions::message_view); shown in the sandboxed frame.
   *  `trackers`: tracking pixels and link trackers removed. */
  body: { html: string; full_html: string | null; text_html: string | null; blocked_remote: number; trackers: number; plain: boolean };
  /** `List-Unsubscribe` as sent: the message can be unsubscribed from. */
  unsubscribe: string | null;
  list_id: string | null;
  one_click: boolean;
  marks: Marks;
  /** When the user unsubscribed from the sender. */
  unsubscribed: number | null;
  sender_trusted: boolean;
  /** The attachments the user sees. */
  files: Attachment[];
  load_error: string | null;
  /** OpenPGP or S/MIME: encrypted, signed, by whom, whether it could be opened. */
  protection: Protection | null;
  /** The files inside an encrypted message (saved through the backend, decrypted again). */
  protected_files: ProtectedFile[];
  /** A meeting invitation, update or cancellation in the message. */
  invitation: InvitationCard | null;
  /** An online meeting's link in the message („Beitreten“). */
  meeting: { url: string; kind: string } | null;
};

// ---------------------------------------------------------------- signed and encrypted mail

export type Protection = {
  method: "pgp" | "smime";
  encrypted: boolean;
  decrypted: boolean;
  signed: boolean;
  signature: "valid" | "untrusted" | "mismatch" | "unknown_key" | "invalid" | "partial" | null;
  signer: string | null;
  fingerprint: string | null;
  error: string | null;
  /** An own key that could open it waits for its passphrase. */
  locked: string | null;
  /** Text outside the signed or encrypted block is shown too. */
  partial: boolean;
};

export type ProtectedFile = { index: number; filename: string | null; mime_type: string; size: number };

export type ProtectMethod = "pgp" | "smime";

export type Protect = { method: ProtectMethod; sign: boolean; encrypt: boolean };

export type Abilities = {
  pgp_sign: boolean;
  pgp_encrypt: boolean;
  pgp_missing: string[];
  smime_sign: boolean;
  smime_encrypt: boolean;
  smime_missing: string[];
  /** Every recipient asked for encrypted mail (Autocrypt): the sheet starts encrypted. */
  pgp_prefer_encrypt: boolean;
};

export type StoredKey = { kind: "pgp" | "smime"; fingerprint: string; email: string; name: string; own: boolean; source: string; expires: number | null };

export type PgpKeyInfo = { fingerprint: string; name: string; email: string; created: number; can_encrypt: boolean };

export type CertInfo = { fingerprint: string; subject: string; issuer: string; emails: string[]; not_after: number };

// ---------------------------------------------------------------- calendar and meetings

export type Busy = "free" | "tentative" | "busy" | "oof" | "elsewhere";

/** How an appointment may be changed here (mail_core::calendar::Editability). */
export type Editability = "yes" | "confirm" | "received" | "series" | "read_only" | "ics";

/** What a change of an appointment says; RFC 3339 times. */
export type CalFields = { title?: string; start?: string; end?: string; location?: string; all_day?: boolean; description?: string };

/** A change waiting for the server. */
export type WriteState = { op: "create" | "update" | "delete"; state: "pending" | "conflict" | "refused"; error: string | null; server: CalFields | null; mine: CalFields | null; send: boolean };

export type CalendarEvent = {
  /** `<calendar>|<object>|<instance>`. */
  key: string;
  calendar_id: number;
  object_id: number;
  uid: string;
  instance: string;
  title: string;
  start: string;
  end: string;
  all_day: boolean;
  location: string;
  organizer: string;
  attendees: string[];
  body: string | null;
  link: string | null;
  link_kind: string | null;
  busy: Busy;
  private: boolean;
  recurring: boolean;
  categories: string[];
  editable: Editability;
  write: WriteState | null;
  /** The organizer moved the meeting since the user answered: the answer is asked for again. */
  reanswer: boolean;
};

export type CalSourceKind = "local" | "caldav" | "graph" | "jmap" | "ics";

export type CalSource = {
  id: number;
  kind: CalSourceKind;
  name: string;
  url: string;
  username: string;
  account_id: number | null;
  auto_schedule: boolean;
  last_sync: number | null;
  error: string | null;
  pending: number;
};

export type CalendarInfo = {
  id: number;
  source_id: number;
  kind: CalSourceKind;
  source_name: string;
  remote_id: string;
  name: string;
  color: string;
  writable: boolean;
  hidden: boolean;
  account_id: number | null;
};

export type CalendarOverview = {
  sources: CalSource[];
  calendars: CalendarInfo[];
  /** Where new appointments go. */
  default: number;
  /** CalDAV servers people use: name and address (`{host}`: the user's own). */
  presets: [string, string][];
  accounts: { account_id: number; email: string; kind: "graph" | "jmap"; on: boolean }[];
};

export type CalUndo = { object_id: number; kind: "created" | "changed" | "deleted"; before: string | null };

export type CalChanged = { event: CalendarEvent | null; undo: CalUndo };

export type Attendee = { name: string; email: string; status: string; role: string };

export type Invitation = {
  method: string;
  uid: string;
  sequence: number;
  recurrence_id: string | null;
  title: string;
  start: string;
  end: string;
  all_day: boolean;
  recurring: boolean;
  location: string;
  description: string | null;
  organizer: string;
  attendees: Attendee[];
  link: string | null;
  link_kind: string | null;
  me: string | null;
  my_status: string | null;
  mine: boolean;
};

export type InvitationAnswer = "accepted" | "tentative" | "declined";

export type InvitationCard = {
  invitation: Invitation;
  answer: InvitationAnswer | null;
  outdated: boolean;
  event_key: string | null;
  conflicts: CalendarEvent[];
  /** The organizer moved the meeting since the user answered. */
  reanswer: boolean;
};

/** An appointment the assistant found, as a draft for the calendar's editor. */
export type EventDraft = { title: string; start: string; end: string; all_day: boolean; location: string; description: string };

export type ArcaloSent = { via_cli: boolean; title: string; markdown: string | null; file_name: string | null };

export type Undo = { outbox: number[]; hidden: number[]; accounts: number[] };

/** A message to send or save (mail_core::compose::Draft). */
export type ComposeDraft = {
  account_id: number;
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  text: string;
  html: string | null;
  attachments: string[];
  in_reply_to: string | null;
  references: string[];
  message_id: string | null;
  reply_to_id: number | null;
  /** The attachments of this message go along (forwarding). */
  forwarded: number[];
  /** Signed and/or encrypted with OpenPGP or S/MIME. */
  protect?: Protect | null;
};

export type Queued = { outbox_id: number; message_id: string; delay: number };

export type FileInfo = { path: string; name: string; size: number; mime_type: string };

export type Contact = { email: string; name: string | null };

export type OutboxEntry = {
  id: number;
  account_id: number;
  kind: string;
  state: "queued" | "running" | "failed" | "parked";
  attempts: number;
  last_error: string | null;
  next_try: number;
};

export type ThemeChoice = "system" | "light" | "dark" | "contrast";
export type LanguageChoice = "system" | "de" | "en";
export type RemoteContent = "ask" | "trusted_senders" | "always";
export type Prefetch = "none" | "recent" | "month";
export type ProxyMode = "none" | "system" | "manual";

/** A network profile (mail_core::network::ProxyProfile). */
export type ProxyProfile = {
  id: string;
  name: string;
  mode: ProxyMode;
  http_proxy: string;
  socks_proxy: string;
  no_proxy: string;
  proxy_user: string;
};

export type RouteGroup = "mail" | "sign_in" | "setup" | "updates" | "ai";

export type NetworkSettings = { profiles: ProxyProfile[]; routes: Partial<Record<RouteGroup, string>> };

export type SystemProxy = { http: string | null; https: string | null; socks: string | null; bypass: string; source: string };

/** What a swipe over a row does on a touch screen. */
export type SwipeAction = "archive" | "delete" | "read" | "flag" | "none";

export type Settings = {
  version: number;
  language: LanguageChoice;
  theme: ThemeChoice;
  remote_content: RemoteContent;
  threads: boolean;
  mark_read_after_secs: number | null;
  notifications: boolean;
  undo_send_secs: number;
  compose_rich_text: boolean;
  show_snippets: boolean;
  prefetch: Prefetch;
  subject_threads: boolean;
  network: NetworkSettings;
  /** Touch screens: a swipe from right to left, and from left to right. */
  swipe_left: SwipeAction;
  swipe_right: SwipeAction;
  /** Phones: look for new mail in the background. */
  background_check: boolean;
  /** First-time senders wait in „Neue Absender“ (off by default). */
  screener: boolean;
  screener_since: number | null;
  /** The inbox shows people and pinned messages only. */
  focus: boolean;
  /** With two accounts or more, „Alle Posteingänge“ opens first. */
  unified_inbox: boolean;
  /** Newsletters as one row per sender. */
  bundles: boolean;
  ai: AiSettings;
  /** Profile pictures from BIMI (on) and Gravatar (off by default). */
  bimi: boolean;
  gravatar: boolean;
  calendar: CalendarSettings;
  /** The own OpenPGP key goes along in every message (Autocrypt). */
  autocrypt: boolean;
  /** New messages are signed with this method when a key or certificate is there. */
  sign_by_default: ProtectMethod | null;
  /** „An Arcalo senden“ makes a page or a task. */
  arcalo_target: "page" | "task";
  /** Desktop updates: installed when the app quits, or only announced. */
  updates: UpdateMode;
  /** „Startanimation“: the mark draws itself while the app loads. */
  startup_animation: boolean;
};

export type UpdateMode = "auto" | "notify";

/** Why a copy offers the release page instead of installing an update itself. */
export type ManualUpdate = "portable" | "package" | "translocated" | "read_only" | "not_writable" | "no_appimage";

export type UpdateInfo = {
  version: string;
  /** Release notes (Markdown) without the testers' checklist. */
  notes: string | null;
  date: string | null;
  /** The release page. */
  url: string;
};

export type UpdateProgress = { downloaded: number; total: number | null; percent: number | null };

export type UpdateStatus = {
  /** A release build with the update key: checking and installing work. */
  enabled: boolean;
  current_version: string;
  available: UpdateInfo | null;
  manual: ManualUpdate | null;
  manual_text: string | null;
  /** The first start after an update (reported once). */
  restarted: { version: string; installed: boolean; from: string | null; notes: string | null; url: string } | null;
  mode: UpdateMode;
  skipped: string | null;
  remind_after: string | null;
  whats_new_seen: string | null;
  download: UpdateProgress & { phase: "idle" | "downloading" | "paused" | "ready" | "failed"; error: string | null };
  /** A downloaded, verified update waits for the quit (its version). */
  ready: string | null;
  /** The install waits for this many messages that are being sent. */
  waiting: number | null;
  release_page: string;
};

/** The Android app's update notice (the APK of the GitHub release; never the Play build). */
export type ApkStatus = {
  enabled: boolean;
  store: boolean;
  current: string;
  phase: "idle" | "checking" | "available" | "blocked" | "error";
  offered: { version: string; notes: string | null; size: number | null; pub_date: string | null; apk: string; page: string } | null;
  error: string | null;
  blocked: string | null;
  last_check: string | null;
  updated_from: string | null;
  notes: string | null;
};

/** „Als Standard-Mail-App festlegen“. */
export type DefaultMail = {
  supported: boolean;
  is_default: boolean | null;
  /** `set`: the app asks the system; `settings`: the system's settings open. */
  how: "set" | "settings";
  note: string | null;
};

/** What an e-mail link (mailto:) asked for. */
export type Mailto = { to: string[]; cc: string[]; bcc: string[]; subject: string; body: string };

export type CalendarSettings = {
  /** Where new appointments go (the first writable calendar without one). */
  default_calendar: number | null;
  /** Length of a new appointment made with a double click or N. */
  entry_minutes: number;
  /** ISO weekdays of the work week (1 = Monday). */
  workdays: number[];
};

// ---------------------------------------------------------------- the AI assistant

export type AiKind = "ollama" | "lm_studio" | "openai" | "anthropic" | "mistral" | "azure" | "compatible";

export type AiProvider = { id: string; name: string; kind: AiKind; base_url: string; model: string; api_version: string };

export type AiSettings = {
  providers: AiProvider[];
  default_provider: string;
  /** Per account id: the provider that may see its mail, or "off". */
  accounts: Record<string, string>;
  auto_summary: boolean;
  priority: boolean;
  /** „Wichtig“ by a cloud provider: every new message of its accounts goes there. Off until switched on. */
  cloud_priority: boolean;
  tone: boolean;
};

/** Where an account's text goes (the privacy line). */
export type Destination = { provider: string | null; name: string; host: string; local: boolean };

export type WritingStyle = "shorter" | "friendlier" | "formal" | "grammar";

export type AiRequest =
  | { task: "summary"; message_id: number }
  | { task: "reply"; message_id: number; instructions: string }
  | { task: "rewrite"; account_id: number; text: string; style: WritingStyle }
  | { task: "translate"; message_id: number; target: string }
  | { task: "extract"; message_id: number }
  | { task: "priority"; message_id: number }
  | { task: "ask"; question: string; provider: string | null };

export type AiSource = { n: number; message_id: number; subject: string; from: string; date: number };

export type AiItem = { kind: "task" | "event"; title: string; date: string | null; time: string | null; end: string | null; place: string | null; at: number | null };

export type AiOutcome = {
  task: AiRequest["task"];
  text: string;
  cancelled: boolean;
  cached: boolean;
  destination: Destination;
  sources: AiSource[];
  items: AiItem[];
  important: boolean | null;
  reason: string | null;
};

export type Tone = {
  greeting: string | null;
  closing: string | null;
  informal: boolean | null;
  words_per_sentence: number;
  samples: string[];
  learned_from: number;
  learned_at: number;
};

// ---------------------------------------------------------------- rules

export type RuleField = "from" | "to" | "cc" | "subject" | "list" | "has_attachment" | "size" | "age" | "category";
export type RuleOp = "contains" | "not_contains" | "is" | "starts_with" | "over" | "under";
export type RuleCondition = { field: RuleField; op: RuleOp; value: string };
export type RuleAction =
  | { kind: "move"; folder: string }
  | { kind: "label"; keyword: string }
  | { kind: "mark_read" }
  | { kind: "flag" }
  | { kind: "pin" }
  | { kind: "forward"; to: string }
  | { kind: "delete" }
  | { kind: "skip_inbox" }
  | { kind: "category"; category: string };

export type Rule = {
  id: number;
  account_id: number | null;
  name: string;
  place: "local" | "server";
  enabled: boolean;
  position: number;
  body: { match_all: boolean; conditions: RuleCondition[]; actions: RuleAction[]; stop: boolean };
  server_ref: string | null;
};

export type RuleServer = { kind: "sieve" | "graph" | "none"; server: Server | null; error: string | null; checked: boolean };

export type RuleReport = { matched: number; removed: number[]; accounts: number[] };

// ---------------------------------------------------------------- templates

export type Template = { id: number; kind: "template" | "snippet"; name: string; trigger: string | null; subject: string; body: string; account_id: number | null };

export type TemplateVars = { name: string; my_name: string; date: string; time: string; subject: string; signature: string };

// ---------------------------------------------------------------- contacts

export type AddressBook = { id: number; kind: "carddav" | "graph"; name: string; url: string; username: string; account_id: number | null; cards: number; last_sync: number | null; error: string | null };

export type CardFile = { id: number; message_id: number; filename: string | null; mime_type: string; size: number; date: number };

export type ContactCard = {
  email: string;
  name: string | null;
  org: string | null;
  phones: string[];
  emails: string[];
  book: string | null;
  received: number;
  sent: number;
  recent: MessageSummary[];
  files: CardFile[];
};

// ---------------------------------------------------------------- the attachment hub

export type FileKind = "images" | "pdf" | "documents" | "spreadsheets" | "presentations" | "archives" | "calendar" | "other";

export type HubFilter = { kind: FileKind | null; sender: string | null; since: number | null; until: number | null; query: string; account_id: number | null };

export type HubItem = { id: number; message_id: number; filename: string | null; mime_type: string; size: number; kind: FileKind; date: number; from: Address | null; subject: string; account_id: number };

export type HubSender = { email: string; name: string | null; files: number };

export type AppInfo = {
  version: string;
  platform: string;
  schema: number;
  locale: string;
  secret_store: { kind: "system" } | { kind: "file"; reason: string } | { kind: "keystore" } | { kind: "unavailable" };
  /** A fixed „now“ in Unix seconds (screenshots only). */
  clock?: number | null;
  /** This build can sign in with Microsoft (a client id was compiled in). */
  microsoft_sign_in?: boolean;
  /** A phone or tablet (or a desktop asked for the touch layout): swipes, pull to refresh. */
  touch?: boolean;
};
