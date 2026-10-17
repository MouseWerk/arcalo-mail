# Architecture

## Layers

```
 ui/ (React, Vite)                ── invoke() ──▶  src-tauri (commands, app state, secrets, platform)
   three panes, compose sheet,                          │
   sandboxed mail frame                                 ▼
                                               crates/mail-core
      ┌──────────────────────────────── engine (one worker per account) ───────────────┐
      │ actions (local first, undo) · compose · autoconfig                               │
      ├──────────┬──────────┬──────────┬───────────┬──────────┬───────────┬──────────┤
      │ account  │ network  │ imap     │ sync      │ store    │ parse     │ outbox   │
      │ (model,  │ (policy, │ (client, │ (strategy,│ (SQLite, │ sanitize  │ (offline │
      │ presets) │ connect, │ fake)    │ machine)  │ FTS5)    │ thread    │ queue)   │
      │ oauth    │ proxies) │ jmap     │ netwatch  │ diag     │ search    │          │
      │          │ smtp     │ (as IMAP)│           │          │           │          │
      └──────────┴──────────┴──────────┴───────────┴──────────┴───────────┴──────────┘
             TLS sockets / HTTPS                         SQLite (WAL, FTS5)
```

As in Arcalo, all logic lives in the core and is tested there; the shell only wires state, events, the credential
store and the platform. The UI never talks to the network or the file system. One crate (`src-tauri`) builds the
desktop app and, through `run()` with `#[cfg_attr(mobile, tauri::mobile_entry_point)]`, the Android and iOS apps.
Identifier: `de.mousewerk.arcalo.mail`.

## Core modules (`crates/mail-core/src`)

| Module | What it does | State |
|---|---|---|
| `account` | Providers (IMAP, Microsoft, Google, JMAP), transports (IMAP+SMTP, Graph, JMAP), servers, auth (password, OAuth 2.0, API token), `Credential` (password or XOAUTH2 token, memory only), presets, validation, credential store names | done for M2 |
| `oauth` | OAuth 2.0 as a public client: PKCE, the authorize address, the loopback redirect (`network::redirect_listener`), code exchange, refresh with rotating refresh tokens (`TokenSource`), Microsoft's errors as `Problem`s (admin consent, a revoked grant); the client id from `ARCALO_MAIL_MS_CLIENT_ID` at build time (docs/microsoft.md); `oauth::fake`, an identity provider for tests | done (Microsoft; Google later) |
| `jmap` | JMAP (RFC 8620/8621): session discovery, Basic or Bearer, method calls, blobs, EventSource push; `JmapSession` implements `ImapClient` (UIDs per folder and MODSEQs kept in the store), so sync and the worker run unchanged; sending with `Email/import` and `EmailSubmission/set`; `jmap::fake` over the fake IMAP server | done |
| `netwatch` | Sleep and network changes from the clocks and the interface addresses; the shell samples, the workers reconnect | done |
| `diag` | A redacted in-memory log and the diagnostics report | done |
| `network` | The only place that opens connections: the policy (TLS or STARTTLS except to loopback, `https://` only), `connect_imap`/`connect_smtp` (tokio-rustls with the platform verifier, or a certificate pinned by its SHA-256 fingerprint after the user trusted it; STARTTLS), `client_for(Service)` (reqwest) and `client_for_events` (long-lived streams), the OAuth redirect listener, SRV and MX lookups, proxy profiles (none, system, manual HTTP CONNECT or SOCKS5 with exceptions; routes per service group `mail`, `sign_in`, `setup`, `updates`, `ai`; IMAP and SMTP take only a SOCKS proxy from the system), and `Problem`, the classified connection error (DNS, refused, timeout, offline, TLS, certificate, sign-in, app password, admin consent, SMTP AUTH off, proxy). A test checks that no other code opens sockets, resolvers or HTTP clients | done |
| `imap` | `ImapClient` trait (capabilities, LIST, SELECT with QRESYNC, header/flag/message FETCH with CHANGEDSINCE, BODYSTRUCTURE and single parts, IDLE, STORE, COPY, MOVE, EXPUNGE, APPEND, SEARCH, CREATE/RENAME/DELETE), `imap::client` (the client on the wire: `imap-proto` parser, LITERAL+, AUTHENTICATE PLAIN or LOGIN, ENABLE QRESYNC), modified UTF-7 folder names, folder roles, and `imap::fake::FakeImap`, an in-memory server with UIDVALIDITY, MODSEQ and vanished UIDs | done |
| `sync` | Per-folder strategy (QRESYNC, CONDSTORE, plain, full resync on UIDVALIDITY change), `sync_mailbox`, `sync_folders`, bodies on demand (whole up to 2 MB, else text, HTML and inline pictures part by part), bodies of the newest 60 messages per folder ahead of time, attachments on demand, server SEARCH, IDLE wait; `SyncMachine`, the I/O-free connection state machine (connect, sync, idle or poll, backoff, sign-in needed, offline) | done |
| `engine` | One worker thread per account (a current-thread runtime with its own store connection): runs the `SyncMachine`, replays the outbox before each round, answers body, attachment and search requests from the UI, and reports `Notice`s (status, changed folders, new mail, outbox, sent); connectors for IMAP/SMTP with a password or OAuth (`NetConnector`) and JMAP (`JmapConnector`); `Command::Reconnect` after sleep or a network change; `check_once`, one round without a worker for the phones' background check | done |
| `actions` | What the UI does, local first: flags, move, archive, spam, delete, folders, send and drafts become outbox steps after a 5 second undo window; `undo` takes them back; `message_view` builds the reading pane's body (sanitized, quote folded, plain text marked) | done |
| `smtp` | Sending with lettre over a `network` stream (submission with STARTTLS or implicit TLS, AUTH PLAIN/LOGIN) and the connection check; a message for a server without 8BITMIME is made 7-bit first (`sevenbit`) | done |
| `sevenbit` | 8-bit parts to quoted-printable or base64, 8-bit headers to RFC 2047 encoded words; addresses with other characters need SMTPUTF8 | done |
| `compose` | Messages built with mail-builder (text and HTML alternative, attachments, reply headers), the plain alternative from the editor's HTML, address parsing | done |
| `autoconfig` | Finding the servers of an address: presets, the provider's autoconfig file, the Thunderbird ISPDB, SRV records, presets by MX, a guess | done |
| `store` | SQLite (WAL, foreign keys), positional migrations through `PRAGMA user_version` (a newer database is refused), accounts, folders with sync state, messages, bodies, attachments, settings | done for M1 |
| `search` | FTS5 query from plain input (prefix words, phrases, `from:`/`subject:`; FTS syntax never passed through), newest first | done |
| `thread` | Conversations from Message-ID, In-Reply-To and References; adopts early replies, merges threads; replies without those headers join by base subject (a reply prefix, 45 days back, setting `subject_threads`) | done |
| `parse` | RFC 5322/MIME with mail-parser: addresses, dates, ids, text and HTML bodies, attachments, preview line | done |
| `sanitize` | ammonia allow-list: no scripts, forms, frames, style sheets or `url()`; remote images removed and counted unless allowed; links get `noopener noreferrer` | done |
| `outbox` | Offline queue: send (and the copy for Sent), flags, move, delete, drafts, folders; strict order per account in two lanes (sending, everything else), due times for the undo window, backoff, parked steps the server refused; a step is claimed by its worker with a lease it renews (never run twice) and carries the UIDVALIDITY of its UIDs | done |
| `triage` | The inbox that sorts itself: a category per message from its header (`classify`: list and unsubscribe headers, `Precedence`, `Auto-Submitted`, machine and transactional senders, receipt words in German and English), decided again when read (a sender's own tab, people for anyone the user wrote to); pages and counts per tab, newsletter bundles per sender, focus (people and pinned), the unified inbox (all accounts); the Screener (`screener_since`, contacts, replies and the user's addresses pass; allowed and blocked senders, blocked mail moved to „Screened Out“ after each inbox sync); subscriptions | done |
| `later` | Time tools: pin and read later by Message-ID (`marks`), snooze (a „Snoozed“ folder on the server, or `snoozed_until` on this device; back unread on top through `bumped_at`), send later (`scheduled`: the finished outbox step), follow-up reminders (due unless someone else answered); `tick` does what is due in one transaction (the shell's clock every 10 s and at start, the phones' background check) | done |
| `unsubscribe` | `List-Unsubscribe` (RFC 2369) of the message the user looks at, read with the URL parser (`plan`): one-click POST (RFC 8058) through `network::client_for(Service::Unsubscribe)`, else a message through the outbox, else the page; always after the user confirmed the exact address, and done only as confirmed (`confirmed_plan`); addresses from mail only to public `https` hosts (`network::check_mail_url`) | done |
| `tracking` | Tracking pixels (one pixel, hidden, known hosts and paths) taken out of the HTML before sanitizing, click redirects unwrapped and campaign parameters removed from links; counted per message; no request | done |
| `ai` | The assistant, as Arcalo's AI layer: `provider` (Ollama, LM Studio, OpenAI, Anthropic, Mistral, Azure OpenAI, any OpenAI-compatible server; endpoints, headers, key names; plain HTTP only to loopback), `client` (the OpenAI chat protocol streamed as server-sent events, cancel through a flag, model lists, readable errors), `prompts` (one builder per task, in the user's language), `assistant` (`prepare` a request: which provider may see the account, what text goes along, the kept answer; `finish`: keep it in `ai_results`), `tone` (greeting, closing, formal or informal and short samples learned from the Sent folder, kept in the settings table), `ask` (keywords, an OR query over FTS5 with bm25, sources and `[n]` citations), `priority` („Wichtig“: header heuristics, then the model), `extract` (tasks and dates as JSON, an .ics entry), `fake` (an OpenAI-compatible provider for tests). `AiSettings::for_account` decides the destination: an account's own choice, else a provider on this computer, never a cloud provider by default | done |
| `rules` | Rules in the app: conditions (from, to, cc, subject, list, attachment, size, age, category; all or any), actions (move, label, mark read, flag, pin, forward, delete, skip inbox, category), order, on/off, stop; `run_new` after each inbox sync (from `engine::sync_round`), `run_on_folder`, `preview`; the server side of each account in `rule_servers` | done |
| `sieve` | Server rules as Sieve (RFC 5228 with `fileinto`, `imap4flags`, `copy`) between `# BEGIN Arcalo Mail` and `# END` markers merged into the active script, so the user's own rules stay; ManageSieve (RFC 5804: STARTTLS, AUTHENTICATE PLAIN, LISTSCRIPTS, GETSCRIPT, PUTSCRIPT, SETACTIVE) through `network::connect_sieve`; `unsupported` says what a server cannot do; `sieve::fake` | done |
| `graph` | Microsoft Graph for Microsoft accounts: inbox rules (`messageRules`) and contacts with photos, with a Graph token from the account's refresh token (docs/microsoft.md) | done |
| `templates` | Templates and snippets (`;;` shortcuts) with placeholders in German and English (`{vorname}`/`{first_name}`, `{datum}`, `{signatur}` …) | done |
| `contacts`, `carddav` | Address books: CardDAV (discovery through the principal and the address book home, ETags, `addressbook-multiget`, vCard 3 and 4 with photos; presets for iCloud, Fastmail, mailbox.org, Posteo, Nextcloud) and Microsoft contacts; suggestions merged with the recipients the app already knows; the contact card (recent mail, files, counts) | done |
| `avatars` | Sender pictures: the address book, then BIMI (only for senders whose mail passed DMARC as the user's own provider wrote in `Authentication-Results`, never for Junk, for the organizational domain; the `default._bimi` TXT record, DMARC with `p=quarantine` or `reject`, the logo checked against the SVG Tiny PS profile and rebuilt from an allow-list, once per domain, cached in `pictures`), then Gravatar when switched on (SHA-256 of the address, `d=404`); nothing from inside a message | done |
| `files` | Attachments on disk: safe names (no bidirectional controls, no trailing dots or spaces), the risk of a type (programs, scripts and shortcuts are never opened, only saved; Office files with macros after a warning), the internet mark (Windows' `Zone.Identifier`, macOS' `com.apple.quarantine`) on every saved or opened file | done |
| `hub` | All attachments of the local copy: kinds by type and name, filters by kind, sender, time and name, senders with counts | done |
| `pop3` | POP3 (RFC 1939, STLS or implicit TLS, USER/PASS, UIDL, RETR, DELE) through `network::connect_pop3`; downloads into local folders (`pop3_messages`, `pop3_folders`), remembers what it took (`pop3_seen`), leaves mail on the server, removes it after N days or at once as chosen; `Pop3Session` implements `ImapClient` over the local tables so sync, actions and the worker run unchanged (its IDLE visits the server every two minutes and on a wake); presets for common providers; `pop3::fake` | done |
| `calendar` | Calendars and appointments: sources (`local`, `caldav`, `graph`, `jmap`, `ics`), calendars with colors, hidden and writable flags, the default one; objects as the server has them (iCalendar, or Graph's JSON) with ETag or change key, expanded per view window (`ics`: RRULE, RDATE, EXDATE, RECURRENCE-ID, VTIMEZONE and IANA zones through `tz`, DST-safe); `events` with what may be changed (`Editability`: own, organized after a confirmation, received, a series instance, read only, a subscription); `create`, `update`, `delete` and `undo` local first, each queued in `cal_writes` (`pending`, `conflict` with both sides, `refused`) and replayed by `sync::sync_source` before it pulls; `resolve` (server version or mine), `discard`. `caldav`: discovery (well-known, principal, calendar home; RFC 6638 auto-schedule), sync-collection with the sync token or ctag and `calendar-multiget`, `PUT`/`DELETE` with `If-Match` (412 is a conflict), presets for iCloud, Fastmail, mailbox.org, Posteo, Nextcloud, `caldav::fake`. `graph`: `/me/calendars`, `calendarView/delta`, writes with the change key in `If-Match`, `accept`/`tentativelyAccept`/`decline`, `graph::fake`. `jmap`: JMAP Calendars (JSCalendar turned into iCalendar), `jmap::fake`. `ics`: subscriptions fetched with their ETag through `network::client_for(Service::Calendar)` | done |
| `itip` | Meetings in mail: the `text/calendar` part of a message (`REQUEST`, `CANCEL`, `REPLY`, `COUNTER`), the card (times, attendees and their answers, conflicts in the user's calendars, outdated by a newer sequence), the answer (`reply_ics`, a `METHOD:REPLY` message from the account), `file` and `apply` (an accepted invitation into the default calendar, updates and cancellations applied by `scan` after new mail, only from the organizer and never from Junk or Trash; a message about one instance of a series changes only that instance, a series update keeps the changed and cancelled instances, an update never changes the user's answer; the same SEQUENCE counts by its DTSTAMP; large messages through the calendar part fetched with their text, `calendar_data`), `update_mail` (a meeting the user organizes, for servers that do not send updates themselves), `draft_from` (the assistant's dates as a draft for the calendar's editor) | done |
| `crypto` | OpenPGP (`pgp`, rPGP: Ed25519 and Curve25519 keys made here, imported armored keys, PGP/MIME sign, encrypt, decrypt, verify, inline cleartext, Autocrypt headers in and out, `wkd`: the Web Key Directory's advanced and direct addresses) and S/MIME (`smime`: PKCS#12 import, CMS SignedData and EnvelopedData with RSA and AES-256-CBC, chains checked against the system's roots with `rustls-webpki`, e-mail protection usage). `protect` wraps a finished message before it is queued; `open` gives the reading pane the inner message and its `Protection`; secrets come through the shell's `Keys` (the credential store); decrypted content is never stored. Keys are used per address (`pgp_key_emails`): an imported key for its user ids, an Autocrypt key only for the address that sent it and only while no newer mail brought another, never before an imported one; protected secret keys need their passphrase (`Keys::pgp_passphrase`); expired and revoked subkeys are skipped, signatures come from a signing subkey; encrypted mail to Bcc recipients goes out as separate copies; S/MIME certificates encrypt only when trusted (a root of the system, or the user trusted it) | done |
| `arcalo` | „An Arcalo senden“: a message as Markdown with a `mid:` link back, the page title or task text, Arcalo's command line found on the PATH | done |
| `widget` | What the Android home-screen widget shows: the unread count and the newest three mails of People (`widget.json`, written by the shell) | done |
| `update`, `update_feed` | Desktop updates without the shell (as Arcalo's): which version is newer, a skipped version and „Später erinnern“, why a copy cannot replace itself (`Manual`: portable, package, macOS translocated or read-only, AppImage not writable), the restart marker and the notes for „Neu in Arcalo Mail“; `latest.json`, the platform entry, a download through `network::client_for(Service::UpdateFeed)` that pauses and continues with `Range`, the minisign check with the version in the trusted comment | done |
| `update_android` | `android-latest.json` of the GitHub APK: whether a release is an update for the installed app (package, version code, signing certificate), never for a store install (`from_store`; an unknown installer counts as a store), the daily check, „Neu in Arcalo Mail“ once | done |
| `mailto` | `mailto:` addresses (RFC 6068) of other apps: recipients, subject and text only | done |
| `settings`, `settings_migrate` | One JSON object with a `version`; ordered, idempotent steps on the raw JSON, unknown keys kept (Arcalo's scheme) | done |
| `i18n` | `tr!`/`trf!` pairs for backend texts in German and English | done |

## Data model (`crates/mail-core/migrations`)

`0001_init.sql` below; `0003_sync_sources.sql` adds `messages.base_subject` and the JMAP mapping (`jmap_state`,
`jmap_mailboxes`, `jmap_emails`: a UID per message and folder, MODSEQs, „gone“ rows for QRESYNC's vanished UIDs);
`0004_inbox_tools.sql` adds `messages.category`, `list_id`, `unsubscribe`, `one_click`, `snoozed_until` and
`bumped_at`, and the tables `senders` (a sender's tab and the Screener's answer), `unsubscribes`, `marks` (pinned,
read later), `snoozes`, `scheduled` (send later) and `followups`.
`0005_power_tools.sql` adds `messages.important` and `important_by` (rules, the model or the user), `followups.kind`
(`no_answer` or `task`) and `note`, and the tables `ai_results` (kept answers per message, task and language),
`rules` and `rule_servers`, `templates`, `address_books`, `cards` and `card_emails`, `pictures` (sender pictures
and BIMI answers per address or domain), and `pop3_messages`, `pop3_folders` and `pop3_seen` (the local folders of
POP3 accounts).
`0006_calendar_crypto.sql` adds `cal_sources` (a source's kind, address, account and sync state; secrets live in
the credential store), `calendars` (remote id, name, color, writable, hidden), `cal_objects` (href, ETag or change
key, UID, `ics` or `graph` data, the first start and last end for the window query, a deleted mark), `cal_writes`
(the offline queue of appointments: create, update or delete, pending, conflict or refused, both sides of a
conflict, whether attendees get the update), `itip_seen` (the invitations of messages and the answers given),
`pgp_keys` and `smime_certs` (public keys and certificates; the secret parts are in the credential store).
`0008_key_trust.sql` adds `pgp_key_emails` (which address each OpenPGP key serves, from where, Autocrypt's
`prefer-encrypt` and date), `smime_certs.trusted` and `messages.dmarc_pass` (the provider's `Authentication-Results`
said DMARC passed for the From's domain; BIMI needs it).
`0009_calendar_ai_keys.sql` adds `cal_writes.version` (changes merged into a write, so one made while it is on its
way stays queued), `prev_op`, `prev_mine` and `prev_send` (what waited before a delete, for „Rückgängig“) and
`strikes` (rounds in which the server answered and the write still failed; then it is parked as refused),
`cal_objects.reanswer` (the organizer moved a meeting the user had answered), `itip_seen.recurrence` (the instance
a message is about) and `user_addresses` (a CalDAV server's `calendar-user-address-set` and a JMAP account's
identities); it drops the assistant's answers kept under row ids (they are keyed by Message-ID and a hash now).
`0002_mail_actions.sql` adds `messages.hidden` (moved or deleted here, not yet on the
server) and `reply_to_json`, `message_bodies.raw` (small messages kept whole, so their attachments open offline),
`attachments.section` and `data`, `accounts.signature`, `contacts` (recipient suggestions) and `remote_senders`
(senders whose images load).

| Table | Purpose |
|---|---|
| `accounts` | Provider, display name, address, sender name, `transport` (JSON of `account::Transport`). No secrets |
| `mailboxes` | Folders per account: name, delimiter, role, selectable, the sync state `uid_validity`, `uid_next`, `highest_modseq`, `last_sync`, and `sync_error` with the next try for a folder the server would not open |
| `messages` | One row per message per folder (`UNIQUE (mailbox_id, uid)`): ids for threading, subject, sender, recipients (JSON), dates, size, flags, keywords, `thread_id`, snippet, `has_body` |
| `message_bodies` | Text and the HTML as sent (sanitized each time it is shown, so a better sanitizer covers old mail), `remote_allowed` |
| `attachments` | Per message: part index, file name, type, size, Content-ID, inline, `local_path` once saved |
| `outbox` | The offline queue (kind, JSON payload, state, attempts, next try, last error, the UIDVALIDITY of its UIDs, the owner and lease of a running step) |
| `message_refs` | Every Message-ID a message refers to (References, In-Reply-To), indexed: threading finds earlier replies without reading every message |
| `messages_fts` | FTS5, contentless with delete support, over subject, people and body text (unicode61, diacritics removed; ß folded to ss by the store) |
| `settings` | Key/value; the app settings are the JSON under `app` |

## Sync

1. Connect (TLS or STARTTLS through `network`), authenticate (password or XOAUTH2 with a fresh access token),
   `CAPABILITY`, `ENABLE QRESYNC` when offered.
2. `LIST` (with SPECIAL-USE): folders are added, updated, removed; folders created or renamed here keep their place
   until the queue brought the change to the server. Roles come from the attributes; a role by name (a top folder,
   or one right below INBOX) only where no folder carries that attribute, once per role.
3. Per folder, inbox first: `SELECT` with `(QRESYNC (uidvalidity modseq))` when known. A different UIDVALIDITY
   clears the folder and fetches it again. Otherwise changed flags and vanished UIDs (QRESYNC), or flags
   `CHANGEDSINCE` plus a UID list when the count says something was removed (CONDSTORE), or flags and UID list
   (plain). New mail: headers above the stored UIDNEXT, in batches of 500. Messages with a flag change still in the
   queue keep the local flags. A folder the server refuses is skipped (its error shown, tried again after 5 minutes,
   doubling up to 6 hours); the other folders go on.
4. Bodies of the newest 60 messages per folder up to 256 KB right after the sync (first lines in the list, search
   over their text, offline reading); others when a message is opened.
5. The inbox connection waits in IDLE (renewed every 25 minutes); other folders are polled every 5 minutes or
   synced when IDLE ends; without IDLE the wait ends early when a queued step is due. Failures back off from 5 s to
   5 minutes (also when connecting works and the rounds fail); a refused login stops until the user signs in, a
   temporary refusal (RFC 5530 `[UNAVAILABLE]`, `[INUSE]`, POP3 `[IN-USE]`) retries by itself.
6. The offline queue replays in order before each sync; the local change is shown at once. A step whose folder has
   another UIDVALIDITY now is parked, never applied to other messages. Without UIDPLUS, `EXPUNGE` keeps the
   `\Deleted` marks of other clients (taken off for the moment and set again). A send whose answer was lost after
   the whole message went over is not sent again by itself: a server that files sent mail shows it in Sent, else
   the step waits for the user.

`SyncMachine` decides these steps and does no I/O itself, so every transition is unit-tested; the shell runs one
per account on a worker and carries out its actions.

## Showing mail safely

- The core sanitizes HTML with an allow-list (`sanitize.rs`) and removes remote images unless the user allowed
  them for the message or its sender. Style attributes keep layout and color properties only, nothing that loads
  (a color given with the `background` shorthand is kept as `background-color`). The UI shows how many images were
  blocked and offers „Bilder laden“ for the message and „Immer für …“ for the sender.
- Tracking pixels and link trackers go first, whatever the user allowed for remote images (`tracking.rs`); the
  reading pane says how many („Tracker entfernt“).
- The reading pane renders the result in an `<iframe sandbox="allow-same-origin">` (no `allow-scripts`, so nothing
  in the mail runs; same origin only lets the app catch link clicks) with `srcdoc` and a meta CSP of its own:
  `default-src 'none'; img-src data: cid:` (plus `https:` once allowed), inline styles only. The app's CSP applies
  on top. Links open in the browser after a click, never inside the frame.
- Inline pictures (`cid:`) are replaced by `data:` addresses of the stored parts in the core, so the frame needs no
  custom scheme. HTML mail is drawn on light paper in every theme (senders design for white); plain text is drawn
  in the app's own colors.

## App CSP (`src-tauri/tauri.conf.json`)

`script-src 'self'` (no inline scripts, no eval), `connect-src` only the IPC, `object-src 'none'`,
`img-src` with `https:` only for remote images the user allowed inside the mail frame (the frame's own CSP blocks
them otherwise). `ui/src/lib/html.test.ts` checks it.

## Secrets

`src-tauri/src/secrets.rs`, one vault per platform:

- Windows (Credential Manager), macOS (Keychain), Linux (Secret Service): the `keyring` crate. On a Linux desktop
  without a Secret Service (or with `ARCALO_MAIL_SECRET_STORE=file`, which the e2e tests use) passwords go to
  `secrets.json` in the data folder with mode 0600, and Settings → Privacy says so.
- Android: an AES-256-GCM key created in the Android Keystore that never leaves it (`KeystoreCipher.kt`, reached
  over JNI from `mobile/android.rs`) seals each secret; only the ciphertexts are kept, in `secrets.sealed.json` in
  the app's private folder (no backups, no device transfer). A value the key cannot open any more reads as missing
  and the account asks for its sign-in again.
- iOS: the Keychain through `security-framework`, items readable after the first unlock and bound to the device
  (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`), so the background check works while the phone is locked.

Entry names: service `de.mousewerk.arcalo.mail`, account `account-<id>-password` (also a JMAP API token),
`account-<id>-oauth-refresh-token` (rotated refresh tokens are written back at once), `proxy-<profile>-password`,
`ai-<provider>-key` (a cloud provider's API key), `carddav-<book>-password`, `calendar-<source>-secret` (a CalDAV
password or the full address of an ICS subscription), `pgp-<fingerprint>-secret` (an armored secret key) and
`smime-<fingerprint>-p12` (the PKCS#12 file and its password).
OAuth access tokens are kept in memory only. The vault logic is tested with a fake cipher.

## Updates and the default mail app (`src-tauri/src/updates.rs`, `installer.rs`, `portable.rs`, `defaultmail.rs`)

`updates.rs` checks the feed at the start and every few hours (release builds with the update key only), downloads
in the background, verifies, and installs on quit or with „Jetzt neu starten“ after messages being sent went out
(`outbox::sending`); `installer.rs` is Arcalo's install without the plugin's own fetch (NSIS passive, the `.app`
bundle, the AppImage in place). `portable.rs`: the marker `arcalo-mail-portable` next to the program keeps the data in
`data\`. `defaultmail.rs` hands `mailto:` addresses to the UI (command line, single instance, macOS open event,
Android intent) and sets the default mail app where the system allows. Releases: [release.md](release.md).

## Phones (`src-tauri/src/mobile`, `src-tauri/gen/android`, `src-tauri/Info.ios.plist`)

The same UI, store, commands and core. What differs:

- **In front or not.** The account workers (IDLE, JMAP push) run only while the app is in front; the UI reports
  `visibilitychange` (`set_in_front`), Android also the activity's pause and resume. In the background the platform
  runs `background::run`: every account once through `engine::check_once` (the offline queue, the inbox, its first
  lines) within a time budget, answering the notifications (one per new message, a summary above four) and the
  unread count. Android: WorkManager about every 15 minutes with a network (`MailCheckWorker.kt`, calling Rust over
  JNI in a process that may have no activity); iOS: a `BGAppRefreshTask` asked for each time the app goes to the
  background, 25 seconds (`mobile/ios.rs`). No push server (docs/plan.md, open decision 5).
- **Badge.** Android: the number of the new-mail notification (launchers show it); iOS: the app icon's number.
- **Notification buttons.** Archive, Mark read and Reply on a new-mail notification. Android: actions handled by
  `MailActionReceiver.kt` (archive and read through `MailNative.notificationAction`, in the running app or on the
  store directly with a check soon after) and an activity intent for Reply; iOS: the category `new-mail` and a
  delegate of the notification center (`mobile/ios.rs`). The desktop: Linux notification actions and Windows toast
  buttons (`notify-rust`); macOS shows the plain notification.
- **Widget (Android).** `MailWidget.kt` reads `widget.json` (`mail_core::widget`), written next to the database when
  the unread count changes and after each background check; a tap opens the app on the message.
- **Sign-in.** Microsoft opens in Custom Tabs (`AuthTab.kt`) or `ASWebAuthenticationSession`; the redirect
  `de.mousewerk.arcalo.mail://oauth` comes back through the activity's intent filter or the session's callback,
  and `oauth::sign_in_with_session` exchanges the code as on the desktop.
- **TLS on Android** checks against Mozilla's roots compiled in (`webpki-root-certs`), as Arcalo's companion app:
  the platform verifier needs a JVM context the background check does not have.
- **Native code without a plugin.** Rust reaches Kotlin over plain JNI (also without Tauri, in the background) and
  iOS's frameworks through the Objective-C runtime (`objc2`), so the iOS project needs no code of its own: it is
  generated in CI, `Info.ios.plist` adds background fetch and the usage strings, `tauri.conf.json` links the
  frameworks, `ios/PrivacyInfo.xcprivacy` is copied in. The Android project is committed (`src-tauri/gen/android`).
- **UI.** `lib/touch.ts` picks the layout (phone: one pane; tablet: list and message, folders in a drawer;
  desktop), the swipes, pull to refresh and what Back does; `lib/viewport.ts` follows the keyboard (visual viewport),
  iOS Dynamic Type and the app's visibility. Touch screens get 44 px targets and one step larger type; the safe areas
  come from `env(safe-area-inset-*)` on iOS and from the activity's padding on Android.

Release steps for the stores: [release-mobile.md](release-mobile.md).

## UI (`ui/`)

Vite + React + TypeScript. `src/styles/tokens.css` is Arcalo's token set (spacing, type, radii, control heights,
motion, layers, the neutral state tints) adapted for mail and extended by a contrast theme; `styles.test.ts`
enforces tokens-only colors, no accent on selected/current/focused states, no side bars and `:focus-visible` rings.
Three panes (`Sidebar`, `MessageList`, `ReadingPane`) and a compose sheet; the calendar (`CalendarView`, Arcalo's
Kalender with its `.calv` styles and the `--ev-tint`/`--ev-meta` tokens, logic in `lib/calendar.ts`) takes the list
and the reading pane; `Meeting.tsx` draws the invitation card, „Beitreten“ and the protection badges; below 720 px one pane at a time with
back navigation; touch screens get the phone and tablet layouts of the section above. In a plain browser `lib/api.ts` serves sample data (`lib/demo.ts`), so the
layout can be checked without the app.

## Chosen crates

| Need | Crate | Why |
|---|---|---|
| IMAP | own client on the `imap-proto` 0.17 parser over `tokio-rustls` | QRESYNC, CONDSTORE, MOVE, LITERAL+ and cancel-safe IDLE without fighting a higher-level API |
| SMTP | `lettre` 0.11 (tokio, rustls) | mature, XOAUTH2, STARTTLS |
| TLS | `rustls` with `aws_lc_rs` and `rustls-platform-verifier` | as Arcalo; system trust store on every platform |
| HTTP (OAuth, Graph, JMAP) | `reqwest` 0.13 with rustls | as Arcalo, through `network::client_for` |
| MIME parsing | `mail-parser` 0.11 | as Arcalo; zero-copy, robust with broken mail |
| MIME building | `mail-builder` (with lettre) | the same author as mail-parser |
| HTML sanitizing | `ammonia` 4 | allow-list on html5ever |
| Store | `rusqlite` 0.40 bundled (FTS5) | as Arcalo; `bundled-sqlcipher` once encryption at rest comes |
| Secrets | `keyring` 3 | as Arcalo |
| DNS (SRV, MX) | `hickory-resolver` | system configuration, async |
| Autoconfig files | `roxmltree` | small, read-only XML |
| Calendars | `chrono`, `chrono-tz`, `rrule`, `iana-time-zone` | time zones and recurrence as calendars write them |
| OpenPGP | `pgp` (rPGP) 0.21 | pure Rust, Ed25519/Curve25519, interoperates with GnuPG |
| S/MIME | `cms` with `x509-cert`, `p12-keystore`, `rsa`, `aes`/`cbc`, `rustls-webpki` | RustCrypto, no OpenSSL |
| Notification buttons | `notify-rust` | Linux actions and Windows toast buttons |

All of them were checked on crates.io; only the ones in use are in `Cargo.toml` now.

## Tests

- `mail-core` unit tests run against `imap::fake::FakeImap` (every sync strategy, the engine with its worker and
  outbox, actions and undo, autoconfig over HTTP on loopback).
- `crates/mail-testserver` serves the fake over the wire: IMAP (with STARTTLS and implicit TLS on a self-signed
  certificate) and SMTP on loopback ports, plus a control port for tests (deliver, flags, offline, fail the next
  command, a slow server, an autoconfig file). `tests/protocol.rs` runs the real client and lettre against it.
- `crates/mail-testserver` also serves the same mailboxes over JMAP (with EventSource push) and an OAuth identity
  provider whose tokens IMAP and SMTP take with XOAUTH2.
- `crates/mail-testserver/tests/conformance.rs` runs the client against real servers, only in CI (the
  `conformance` job starts Dovecot and GreenMail containers); locally it is ignored.
- `e2e/` starts the built app under tauri-driver (WebdriverIO, as Arcalo) against the test server program:
  setup, reading, writing, search, offline, folders and bulk actions, Microsoft sign-in (the test follows the
  address the app opened), the Gmail guide and JMAP, the sorted inbox (tabs, bundles, focus, moving a sender,
  trackers, one-click unsubscribe to the test server's web side), the Screener, the unified inbox, snooze, send
  later (also at the next start) and reminders (`ARCALO_MAIL_TICK_SECS=1` speeds up the clock), the assistant
  against the test server's fake OpenAI-compatible provider (`/ai/v1`: streaming, stop, errors, an account no
  provider may see), rules in the app and over ManageSieve (the test server's Sieve port), templates and snippets,
  CardDAV contacts (`/dav`) with the contact card, a POP3 account set up by hand, the attachments, the calendar
  against the test server's CalDAV calendars (`/caldav`: create, move, delete, undo, hide, the views), meeting
  invitations with the iTIP answer in what was sent, „Beitreten“ and „An Arcalo senden“ (a stand-in `arcalo` on the
  PATH), OpenPGP against the test server's own key (`/pgp`: Jonas opens what the app sent and answers encrypted),
  and the new views on a phone and a tablet. `e2e/shots` writes the README screenshots from English sample
  mailboxes on a fixed clock (`ARCALO_MAIL_NOW`).
