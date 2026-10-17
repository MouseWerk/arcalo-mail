-- The phishing warning (`phishing`): what the user's provider found failing for a message
-- (`Authentication-Results`: `dmarc`, `spf`, separated by spaces), and the senders the user said
-- are fine („Das ist in Ordnung“).
ALTER TABLE messages ADD COLUMN auth_fail TEXT;

CREATE TABLE phishing_ok (
  email TEXT PRIMARY KEY COLLATE NOCASE,
  at INTEGER NOT NULL
);
