-- Arcalo Mail, schema version 1: accounts, folders, messages with threads, attachments, the
-- offline queue and the full-text index. Migrations are positional (0002_… next, never fill a
-- gap); `PRAGMA user_version` holds the number of the last one applied.

-- Server settings only. Passwords and tokens are in the OS credential store
-- (`account::secret_key`), never here.
CREATE TABLE accounts (
  id INTEGER PRIMARY KEY,
  provider TEXT NOT NULL CHECK (provider IN ('imap', 'microsoft', 'google', 'jmap')),
  name TEXT NOT NULL DEFAULT '',
  email TEXT NOT NULL,
  sender_name TEXT NOT NULL DEFAULT '',
  -- `account::Transport` as JSON.
  transport TEXT NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- Folders and their sync state (RFC 7162: UIDVALIDITY, UIDNEXT, HIGHESTMODSEQ).
CREATE TABLE mailboxes (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  delimiter TEXT,
  -- `imap::Role`: inbox, sent, drafts, trash, junk, archive, all, flagged; NULL for the user's own.
  role TEXT,
  selectable INTEGER NOT NULL DEFAULT 1,
  uid_validity INTEGER,
  uid_next INTEGER,
  highest_modseq INTEGER,
  last_sync INTEGER,
  UNIQUE (account_id, name)
);

-- One row per message per folder (the same message in two folders is two rows with one thread).
CREATE TABLE messages (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  modseq INTEGER,
  -- `Message-ID` without angle brackets; threads are built from it (`thread.rs`).
  message_id TEXT,
  in_reply_to TEXT,
  -- `References` as a JSON array.
  refs TEXT NOT NULL DEFAULT '[]',
  thread_id INTEGER NOT NULL DEFAULT 0,
  subject TEXT NOT NULL DEFAULT '',
  from_name TEXT,
  from_email TEXT,
  -- Recipients as JSON arrays of `parse::Address`.
  to_json TEXT NOT NULL DEFAULT '[]',
  cc_json TEXT NOT NULL DEFAULT '[]',
  date INTEGER,
  internal_date INTEGER NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  seen INTEGER NOT NULL DEFAULT 0,
  answered INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  draft INTEGER NOT NULL DEFAULT 0,
  -- Other IMAP keywords as a JSON array.
  keywords TEXT NOT NULL DEFAULT '[]',
  has_attachments INTEGER NOT NULL DEFAULT 0,
  snippet TEXT NOT NULL DEFAULT '',
  -- 0: header only, 1: body stored in `message_bodies`.
  has_body INTEGER NOT NULL DEFAULT 0,
  UNIQUE (mailbox_id, uid)
);
CREATE INDEX messages_list ON messages (mailbox_id, internal_date DESC);
CREATE INDEX messages_thread ON messages (thread_id);
CREATE INDEX messages_msgid ON messages (account_id, message_id);
CREATE INDEX messages_reply ON messages (account_id, in_reply_to);

-- Bodies are loaded on demand (opening a message, or the offline download of recent mail).
CREATE TABLE message_bodies (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  text TEXT,
  -- As sent; sanitized each time it is shown (`sanitize.rs`), so a better sanitizer applies to old mail.
  html TEXT,
  -- Remote content allowed for this message.
  remote_allowed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE attachments (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_index INTEGER NOT NULL,
  filename TEXT,
  mime_type TEXT NOT NULL,
  size INTEGER NOT NULL,
  content_id TEXT,
  inline INTEGER NOT NULL DEFAULT 0,
  -- Where the file was saved once downloaded (in the app's data folder), else NULL.
  local_path TEXT
);
CREATE INDEX attachments_message ON attachments (message_id);

-- The offline queue: changes made without a connection, replayed in order (`outbox.rs`).
CREATE TABLE outbox (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  -- send, flags, move, delete, append
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  -- queued, running, failed
  state TEXT NOT NULL DEFAULT 'queued',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_try INTEGER NOT NULL,
  last_error TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX outbox_due ON outbox (state, next_try);

-- Full-text search over subject, people and text (unicode61, diacritics removed: „Köln“ finds
-- „koln“; `search.rs` also folds ß to ss on both sides). External content: the body text comes from `message_bodies`, so the index is
-- written by the store (`store.rs`, `index_message`), not by triggers.
CREATE VIRTUAL TABLE messages_fts USING fts5 (
  subject,
  people,
  body,
  content = '',
  contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
