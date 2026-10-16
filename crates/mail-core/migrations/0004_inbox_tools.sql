-- Arcalo Mail, schema version 4: the inbox that sorts itself (categories, the Screener,
-- subscriptions) and the time tools (snooze, send later, follow-up reminders, pin, read later).

-- What the header says about a message (`triage::classify`): people, newsletters,
-- notifications, receipts, other. The lists show the sender's own category instead when the user
-- moved the sender, and people for anyone the user wrote to.
ALTER TABLE messages ADD COLUMN category TEXT NOT NULL DEFAULT 'people';
-- `List-Id` without brackets, `List-Unsubscribe` as sent, and whether that takes the one-click
-- POST of RFC 8058 (`List-Unsubscribe-Post`).
ALTER TABLE messages ADD COLUMN list_id TEXT;
ALTER TABLE messages ADD COLUMN unsubscribe TEXT;
ALTER TABLE messages ADD COLUMN one_click INTEGER NOT NULL DEFAULT 0;
-- Snoozed on this device only (no Message-ID to find it again on the server): left out of the
-- inbox until then.
ALTER TABLE messages ADD COLUMN snoozed_until INTEGER;
-- Back from snoozing, or a reminder that came due: sorted as if it arrived then.
ALTER TABLE messages ADD COLUMN bumped_at INTEGER;
CREATE INDEX messages_sender ON messages (from_email COLLATE NOCASE);

-- Per sender: the category the user moved it to (remembered for every message of the sender),
-- and the Screener's answer (`allowed`, `blocked`; NULL: not asked yet).
CREATE TABLE senders (
  email TEXT PRIMARY KEY COLLATE NOCASE,
  category TEXT,
  screener TEXT,
  updated_at INTEGER NOT NULL
);

-- Senders the user unsubscribed from, and how (`one_click`, `mailto`, `link`).
CREATE TABLE unsubscribes (
  sender TEXT PRIMARY KEY COLLATE NOCASE,
  method TEXT NOT NULL,
  at INTEGER NOT NULL
);

-- Pinned and read later, by Message-ID (`local:<id>` without one), so a copy in another folder
-- keeps them.
CREATE TABLE marks (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_key TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0,
  later_at INTEGER,
  PRIMARY KEY (account_id, message_key)
);

-- Snoozed messages. With a folder they wait in it on the server (other clients see them out of
-- the inbox) and come back to `mailbox` when due; without one only here (`messages.snoozed_until`).
CREATE TABLE snoozes (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_key TEXT NOT NULL,
  until INTEGER NOT NULL,
  mailbox TEXT NOT NULL,
  folder TEXT,
  -- The queued move into the folder, taken back when the message is due before it ran.
  outbox_id INTEGER,
  -- waiting, returned
  state TEXT NOT NULL DEFAULT 'waiting',
  returned_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX snoozes_due ON snoozes (state, until);
CREATE INDEX snoozes_key ON snoozes (account_id, message_key);

-- Messages to send later: the finished outbox step, queued when it is due (on the desktop while
-- the app runs, else at its next start; on a phone by the background check).
CREATE TABLE scheduled (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  due_at INTEGER NOT NULL,
  -- `outbox::Op::Send` as JSON.
  op TEXT NOT NULL,
  -- `compose::Draft` as JSON, to open it again.
  draft TEXT NOT NULL,
  subject TEXT NOT NULL DEFAULT '',
  recipients TEXT NOT NULL DEFAULT '',
  message_id TEXT NOT NULL,
  -- A follow-up reminder this many seconds after it went out, or NULL.
  followup_secs INTEGER,
  created_at INTEGER NOT NULL
);

-- „Erinnern, wenn keine Antwort“: due unless an answer to the message arrived by then.
CREATE TABLE followups (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_key TEXT NOT NULL,
  due_at INTEGER NOT NULL,
  subject TEXT NOT NULL DEFAULT '',
  -- waiting, due (shown to the user), done
  state TEXT NOT NULL DEFAULT 'waiting',
  created_at INTEGER NOT NULL
);
CREATE INDEX followups_due ON followups (state, due_at);
