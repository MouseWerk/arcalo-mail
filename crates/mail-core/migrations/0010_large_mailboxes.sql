-- Gmail's X-GM-MSGID: the copies of one message in its label folders and in All Mail share it,
-- so search and the bodies treat them as one message.
ALTER TABLE messages ADD COLUMN gm_msgid INTEGER;
CREATE INDEX messages_gm ON messages (account_id, gm_msgid) WHERE gm_msgid IS NOT NULL;

-- A first sync fetches the newest headers of a folder; the older ones follow a part per round.
-- `backfill_uid`: the lowest UID fetched so far while older mail waits (NULL: nothing waits);
-- `backfill_left`: how many older messages the server had at the last look.
ALTER TABLE mailboxes ADD COLUMN backfill_uid INTEGER;
ALTER TABLE mailboxes ADD COLUMN backfill_left INTEGER NOT NULL DEFAULT 0;
