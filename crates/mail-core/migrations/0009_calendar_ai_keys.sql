-- Calendar writes: `version` counts the changes merged into a write, so a change made while
-- the write is on its way stays queued when the server takes the earlier one. A delete merged
-- into a waiting change keeps that change (`prev_op`, `prev_mine`, `prev_send`), so „Rückgängig“
-- brings it back. `strikes`: rounds in which the server was reachable and the write still
-- failed; after a few the write is parked as refused and waits for „Verwerfen“.
ALTER TABLE cal_writes ADD COLUMN version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cal_writes ADD COLUMN prev_op TEXT;
ALTER TABLE cal_writes ADD COLUMN prev_mine TEXT;
ALTER TABLE cal_writes ADD COLUMN prev_send INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cal_writes ADD COLUMN strikes INTEGER NOT NULL DEFAULT 0;

-- The organizer moved a meeting the user had answered: the answer stays as it was (nothing is
-- sent for the user), the appointment asks for a new one.
ALTER TABLE cal_objects ADD COLUMN reanswer INTEGER NOT NULL DEFAULT 0;

-- The instance of a series an invitation's message is about (its RECURRENCE-ID, empty for the
-- whole series): a change of one instance does not make the series' invitation outdated.
ALTER TABLE itip_seen ADD COLUMN recurrence TEXT NOT NULL DEFAULT '';

-- The user's further addresses: a CalDAV server's calendar-user-address-set and a JMAP account's
-- sender identities, so invitations to an alias are recognized as the user's.
CREATE TABLE user_addresses (
  id INTEGER PRIMARY KEY,
  address TEXT NOT NULL,
  cal_source_id INTEGER REFERENCES cal_sources(id) ON DELETE CASCADE,
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE INDEX user_addresses_address ON user_addresses (address);

-- The assistant's kept answers were keyed by the message's row id, which a later message can
-- take; they are keyed by Message-ID and a hash of the text now, the old ones are dropped.
DELETE FROM ai_results WHERE key LIKE 'msg:%' OR key LIKE 'thread:%';
