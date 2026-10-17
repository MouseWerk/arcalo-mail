-- Arcalo Mail, schema version 6: the calendar (sources, calendars, their objects and the changes
-- waiting for the server), the invitations seen in mail, OpenPGP keys and S/MIME certificates.

-- Where calendars come from: this device (`local`), a CalDAV server (`caldav`), a Microsoft
-- account's calendars (`graph`), a JMAP account's calendars (`jmap`) or an ICS subscription
-- (`ics`). Secrets (a CalDAV password, a subscription's address) live in the credential store
-- (`calendar-<id>-secret`); `url` of a subscription holds its scheme and host only.
CREATE TABLE cal_sources (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  url TEXT NOT NULL DEFAULT '',
  username TEXT NOT NULL DEFAULT '',
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
  -- The CalDAV server sends invitations and updates to attendees itself (RFC 6638).
  auto_schedule INTEGER NOT NULL DEFAULT 0,
  last_sync INTEGER,
  error TEXT,
  created_at INTEGER NOT NULL
);

-- The calendars of a source: the server's id (a CalDAV collection's address, a Graph or JMAP
-- id), the name, the color (`own_color`: chosen by the user, kept over the server's), whether
-- the user may write and whether it shows; `sync_state` is the CalDAV sync token, Graph's delta
-- link, the JMAP state or a subscription's ETag.
CREATE TABLE calendars (
  id INTEGER PRIMARY KEY,
  source_id INTEGER NOT NULL REFERENCES cal_sources(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  name TEXT NOT NULL,
  color TEXT NOT NULL,
  own_color INTEGER NOT NULL DEFAULT 0,
  writable INTEGER NOT NULL DEFAULT 1,
  hidden INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL DEFAULT 0,
  sync_state TEXT,
  ctag TEXT,
  UNIQUE (source_id, remote_id)
);

-- The objects of a calendar as the server has them: iCalendar text (`ics`) or Graph's event
-- JSON (`graph`), with the ETag or change key a write is based on. `first_start`/`last_end`
-- (Unix seconds, NULL: open) pick the objects of a range before series are expanded. `deleted`:
-- deleted here, the server not told yet.
CREATE TABLE cal_objects (
  id INTEGER PRIMARY KEY,
  calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
  href TEXT NOT NULL,
  etag TEXT,
  uid TEXT NOT NULL DEFAULT '',
  format TEXT NOT NULL DEFAULT 'ics',
  data TEXT NOT NULL,
  first_start INTEGER,
  last_end INTEGER,
  deleted INTEGER NOT NULL DEFAULT 0,
  UNIQUE (calendar_id, href)
);
CREATE INDEX cal_objects_uid ON cal_objects (uid);
CREATE INDEX cal_objects_range ON cal_objects (first_start, last_end);

-- Changes made here, one per object (`create`, `update`, `delete`), until the server took them:
-- `pending`, `conflict` (the server's copy changed meanwhile, in `server`; the user decides) or
-- `refused`. `mine`: the fields changed here (JSON); `send`: the user confirmed that a meeting's
-- attendees get the update.
CREATE TABLE cal_writes (
  id INTEGER PRIMARY KEY,
  object_id INTEGER NOT NULL UNIQUE REFERENCES cal_objects(id) ON DELETE CASCADE,
  op TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  send INTEGER NOT NULL DEFAULT 0,
  mine TEXT,
  server TEXT,
  error TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- Invitations (iTIP in mail) the app has seen: the appointment's UID and SEQUENCE, the method
-- and the user's answer, so an update is applied once and the card shows the answer.
CREATE TABLE itip_seen (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  uid TEXT NOT NULL,
  method TEXT NOT NULL,
  sequence INTEGER NOT NULL DEFAULT 0,
  answer TEXT,
  applied INTEGER NOT NULL DEFAULT 0,
  at INTEGER NOT NULL
);
CREATE INDEX itip_seen_uid ON itip_seen (uid);

-- OpenPGP keys: the user's own (the secret key in the credential store, `pgp-<fingerprint>`) and
-- other people's public keys (imported, from Autocrypt headers, from a Web Key Directory).
CREATE TABLE pgp_keys (
  fingerprint TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  public TEXT NOT NULL,
  own INTEGER NOT NULL DEFAULT 0,
  source TEXT NOT NULL,
  prefer_encrypt INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX pgp_keys_email ON pgp_keys (email);

-- S/MIME certificates: the user's own (key and certificate as PKCS#12 in the credential store,
-- `smime-<fingerprint>`) and other people's, taken from their signed mail.
CREATE TABLE smime_certs (
  fingerprint TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  subject TEXT NOT NULL,
  issuer TEXT NOT NULL,
  not_after INTEGER NOT NULL,
  der BLOB NOT NULL,
  own INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX smime_certs_email ON smime_certs (email);
