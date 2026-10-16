-- Arcalo Mail, schema version 3 (milestone 2): JMAP accounts on the same sync engine, and
-- conversations by subject for mail without reply headers.

-- The subject without „Re:“, „AW:“, „Fwd:“ … and in lower case (`thread::base_subject`), for
-- replies that lost their In-Reply-To and References. Filled for messages stored from now on.
ALTER TABLE messages ADD COLUMN base_subject TEXT;
CREATE INDEX messages_base_subject ON messages (account_id, base_subject, date);

-- JMAP (RFC 8620/8621) has string ids and one id per message in several folders; the sync
-- engine speaks IMAP's terms. `jmap.rs` gives every message in every folder a UID of its own,
-- counted up per folder as IMAP does, and a MODSEQ from an account-wide counter, so QRESYNC's
-- logic (changed since, vanished since) answers from these tables.
CREATE TABLE jmap_state (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  -- The `state` of Email the tables are in line with (`Email/changes` starts there).
  email_state TEXT,
  mailbox_state TEXT,
  -- The last MODSEQ handed out.
  modseq INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE jmap_mailboxes (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  -- The JMAP Mailbox id.
  mailbox TEXT NOT NULL,
  uid_next INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (account_id, mailbox)
);

CREATE TABLE jmap_emails (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox TEXT NOT NULL,
  email_id TEXT NOT NULL,
  uid INTEGER NOT NULL,
  modseq INTEGER NOT NULL,
  -- The JMAP keywords as a JSON array, sorted.
  keywords TEXT NOT NULL DEFAULT '[]',
  -- Left the folder (moved, deleted) at `modseq`; kept for QRESYNC's vanished UIDs.
  gone INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, mailbox, email_id)
);
CREATE INDEX jmap_emails_uid ON jmap_emails (account_id, mailbox, uid);
CREATE INDEX jmap_emails_email ON jmap_emails (account_id, email_id);
