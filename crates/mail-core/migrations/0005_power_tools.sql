-- Arcalo Mail, schema version 5: the AI assistant, rules, templates and snippets, contacts with
-- profile pictures, POP3 accounts and reminders for tasks found in a message.

-- Priority („Wichtig“): NULL not decided yet, 0 or 1; decided by the local heuristics
-- (`rules`) or by the AI model (`model`), which may overrule them.
ALTER TABLE messages ADD COLUMN important INTEGER;
ALTER TABLE messages ADD COLUMN important_by TEXT;

-- What the AI assistant answered, kept on this device so a summary is not asked for twice:
-- per thread or message (`key`), task (`summary`, `translation:<lang>`, `extract`) and language.
CREATE TABLE ai_results (
  key TEXT NOT NULL,
  kind TEXT NOT NULL,
  lang TEXT NOT NULL,
  provider TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (key, kind, lang)
);

-- Rules: conditions and actions as JSON (`rules::Rule`). `place` local: the app runs it on new
-- mail; server: it lives on the server (Sieve or Microsoft inbox rules) and the app keeps the
-- copy it uploaded. `account_id` NULL: every account (local rules only).
CREATE TABLE rules (
  id INTEGER PRIMARY KEY,
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  place TEXT NOT NULL DEFAULT 'local',
  enabled INTEGER NOT NULL DEFAULT 1,
  position INTEGER NOT NULL DEFAULT 0,
  body TEXT NOT NULL,
  -- The id of the rule on the server (Microsoft's messageRule id).
  server_ref TEXT,
  updated_at INTEGER NOT NULL
);
CREATE INDEX rules_order ON rules (position, id);

-- Where an account's server rules live: `sieve` (ManageSieve, RFC 5804) with its server as
-- JSON, `graph` (Microsoft inbox rules), `none`; the script the app last uploaded, the error of
-- the last attempt.
CREATE TABLE rule_servers (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  server TEXT,
  script TEXT,
  checked_at INTEGER NOT NULL,
  error TEXT
);

-- Templates (a subject and a text with variables) and snippets (a text behind a short trigger
-- word), for every account or one.
CREATE TABLE templates (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'template',
  name TEXT NOT NULL,
  trigger TEXT,
  subject TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '',
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
  updated_at INTEGER NOT NULL
);

-- Address books: CardDAV collections (iCloud, Fastmail, mailbox.org, Nextcloud …; the password
-- in the credential store) and the contacts of a Microsoft account (Graph).
CREATE TABLE address_books (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL DEFAULT '',
  username TEXT NOT NULL DEFAULT '',
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
  last_sync INTEGER,
  error TEXT,
  created_at INTEGER NOT NULL
);

-- One contact of an address book (a vCard, or a Graph contact), with its picture.
CREATE TABLE cards (
  id INTEGER PRIMARY KEY,
  book_id INTEGER NOT NULL REFERENCES address_books(id) ON DELETE CASCADE,
  href TEXT NOT NULL,
  etag TEXT,
  name TEXT NOT NULL DEFAULT '',
  emails TEXT NOT NULL DEFAULT '[]',
  phones TEXT NOT NULL DEFAULT '[]',
  org TEXT,
  photo BLOB,
  photo_type TEXT,
  UNIQUE (book_id, href)
);
CREATE TABLE card_emails (
  email TEXT NOT NULL COLLATE NOCASE,
  card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  PRIMARY KEY (email, card_id)
);

-- Profile pictures fetched once and kept: `bimi:<domain>` (the sender's verified logo),
-- `gravatar:<address>` when switched on. `data` NULL: there is none (asked again after a while).
CREATE TABLE pictures (
  key TEXT PRIMARY KEY,
  mime TEXT,
  data BLOB,
  fetched_at INTEGER NOT NULL
);

-- POP3 accounts: the messages as downloaded, in the folders the app keeps for them (the server
-- has only the inbox). `uidl` ties a message to the server's copy while it is there.
CREATE TABLE pop3_messages (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox TEXT NOT NULL,
  uid INTEGER NOT NULL,
  uidl TEXT,
  raw BLOB NOT NULL,
  seen INTEGER NOT NULL DEFAULT 0,
  answered INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  draft INTEGER NOT NULL DEFAULT 0,
  keywords TEXT NOT NULL DEFAULT '[]',
  internal_date INTEGER NOT NULL,
  modseq INTEGER NOT NULL DEFAULT 1,
  UNIQUE (account_id, mailbox, uid)
);
CREATE INDEX pop3_uidl ON pop3_messages (account_id, uidl);
-- The folders of a POP3 account, their next UID and change counter; and the UIDLs seen on the
-- server with when they were first seen (for „delete from the server after N days“) and
-- whether the user deleted the message here.
CREATE TABLE pop3_folders (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  uid_next INTEGER NOT NULL DEFAULT 1,
  modseq INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (account_id, name)
);
CREATE TABLE pop3_seen (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  uidl TEXT NOT NULL,
  first_seen INTEGER NOT NULL,
  -- Deleted here: removed from the server at the next visit.
  deleted INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, uidl)
);

-- Reminders of a task found in a message: due at their time whatever happened (a follow-up
-- reminder is due only when nobody answered).
ALTER TABLE followups ADD COLUMN kind TEXT NOT NULL DEFAULT 'no_answer';
ALTER TABLE followups ADD COLUMN note TEXT;
