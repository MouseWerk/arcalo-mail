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
  | { kind: "jmap"; session_url: string; username: string; auth: Auth };

export type ImapTransport = Extract<Transport, { kind: "imap" }>;

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
  | { kind: "proxy"; proxy: string; detail: string };

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
};

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
  /** Sanitized in the backend (mail_core::actions::message_view); shown in the sandboxed frame. */
  body: { html: string; full_html: string | null; text_html: string | null; blocked_remote: number; plain: boolean };
  sender_trusted: boolean;
  /** The attachments the user sees. */
  files: Attachment[];
  load_error: string | null;
};

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

export type RouteGroup = "mail" | "sign_in" | "setup" | "updates";

export type NetworkSettings = { profiles: ProxyProfile[]; routes: Partial<Record<RouteGroup, string>> };

export type SystemProxy = { http: string | null; https: string | null; socks: string | null; bypass: string; source: string };

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
};

export type AppInfo = {
  version: string;
  platform: string;
  schema: number;
  locale: string;
  secret_store: { kind: "system" } | { kind: "file"; reason: string } | { kind: "unavailable" };
  /** A fixed „now“ in Unix seconds (screenshots only). */
  clock?: number | null;
  /** This build can sign in with Microsoft (a client id was compiled in). */
  microsoft_sign_in?: boolean;
};
