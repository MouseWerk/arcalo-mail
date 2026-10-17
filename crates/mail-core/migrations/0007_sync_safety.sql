-- Arcalo Mail, schema version 7: the offline queue's steps are claimed by the worker that runs
-- them and remember the UIDVALIDITY of their UIDs, folders the server will not open are skipped for a while, and the references of
-- messages are indexed for threading.

-- A running step belongs to its worker: `owner` names the process, `lease_until` is renewed
-- while it runs. Another worker never takes a step whose lease is alive (no message goes out
-- twice); one of a process that ended runs again.
ALTER TABLE outbox ADD COLUMN owner TEXT;
ALTER TABLE outbox ADD COLUMN lease_until INTEGER;
-- The UIDVALIDITY of the folder whose UIDs a step names, as it was when the step was queued: a
-- step never acts on the UIDs of another one (the folder was rebuilt on the server).
ALTER TABLE outbox ADD COLUMN uid_validity INTEGER;

-- A folder the server lists but refused to open: its error, how often in a row, and when the
-- next round tries again.
ALTER TABLE mailboxes ADD COLUMN sync_error TEXT;
ALTER TABLE mailboxes ADD COLUMN sync_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mailboxes ADD COLUMN sync_retry_at INTEGER;

-- Every Message-ID a message refers to (`References` and `In-Reply-To`): a new message finds
-- the replies that came before it through the index instead of reading every message.
CREATE TABLE message_refs (
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  ref TEXT NOT NULL,
  PRIMARY KEY (message_id, ref)
) WITHOUT ROWID;
CREATE INDEX message_refs_ref ON message_refs (account_id, ref);
INSERT OR IGNORE INTO message_refs (message_id, account_id, ref)
  SELECT m.id, m.account_id, j.value FROM messages m, json_each(m.refs) j
  WHERE json_valid(m.refs) AND j.type = 'text' AND j.value <> '';
INSERT OR IGNORE INTO message_refs (message_id, account_id, ref)
  SELECT id, account_id, in_reply_to FROM messages WHERE in_reply_to IS NOT NULL AND in_reply_to <> '';
