// The shapes the commands return (src-tauri/src/commands.rs, mail_core's types).

export type Provider = "imap" | "microsoft" | "google" | "jmap";
export type Role = "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "all" | "flagged";

export type Address = { name: string | null; email: string };

export type Account = { id: number; provider: Provider; name: string; email: string; sender_name: string };

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
};

export type MessageView = {
  summary: MessageSummary;
  to: Address[];
  cc: Address[];
  message_id: string | null;
  has_body: boolean;
  text: string | null;
  remote_allowed: boolean;
  /** Sanitized in the backend (mail_core::sanitize); shown in the sandboxed frame. */
  body: { html: string; blocked_remote: number };
};

export type ThemeChoice = "system" | "light" | "dark" | "contrast";
export type LanguageChoice = "system" | "de" | "en";

export type Settings = {
  version: number;
  language: LanguageChoice;
  theme: ThemeChoice;
  remote_content: "ask" | "trusted_senders";
  threads: boolean;
  mark_read_after_secs: number | null;
};

export type ComposeDraft = { account_id: number; to: string[]; cc: string[]; subject: string; body: string };
