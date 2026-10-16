-- Arcalo Mail, schema version 2: what milestone 1 needs for real accounts. Moves and deletions
-- that wait for the server, whole messages kept for offline attachments, parts of large
-- messages fetched on demand, signatures, Reply-To, the address book of the compose sheet and
-- the senders whose remote images load.

-- Moved or deleted here, not yet on the server (the offline queue holds the change). Hidden
-- rows are left out of lists, counts and search; undo shows them again.
ALTER TABLE messages ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
-- `Reply-To` as a JSON array of `parse::Address` (replies go there when it is set).
ALTER TABLE messages ADD COLUMN reply_to_json TEXT NOT NULL DEFAULT '[]';
CREATE INDEX messages_visible ON messages (mailbox_id, hidden, internal_date DESC);

-- The whole message as sent, when it was fetched whole (up to `sync::FULL_FETCH_LIMIT`): its
-- attachments open offline. Larger messages keep their text only; their attachments are
-- fetched by `attachments.section` when opened.
ALTER TABLE message_bodies ADD COLUMN raw BLOB;
ALTER TABLE attachments ADD COLUMN section TEXT;
-- Pictures a large HTML message shows (`cid:`), fetched with its text.
ALTER TABLE attachments ADD COLUMN data BLOB;

-- Plain text added below new messages and replies of the account.
ALTER TABLE accounts ADD COLUMN signature TEXT NOT NULL DEFAULT '';

-- Addresses for the autocomplete of To, Cc and Bcc: everyone who wrote, and everyone the user
-- wrote to (`sent`, ranked higher).
CREATE TABLE contacts (
  email TEXT PRIMARY KEY COLLATE NOCASE,
  name TEXT,
  received INTEGER NOT NULL DEFAULT 0,
  sent INTEGER NOT NULL DEFAULT 0,
  last_used INTEGER NOT NULL DEFAULT 0
);
INSERT INTO contacts (email, name, received, last_used)
  SELECT from_email, MAX(from_name), COUNT(*), MAX(internal_date) FROM messages
  WHERE from_email IS NOT NULL AND from_email <> '' GROUP BY from_email COLLATE NOCASE;

-- Senders whose remote images always load („Immer für diesen Absender“).
CREATE TABLE remote_senders (
  email TEXT PRIMARY KEY COLLATE NOCASE,
  added_at INTEGER NOT NULL
);
