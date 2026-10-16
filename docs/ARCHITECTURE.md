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
| `network` | The only place that opens connections: the policy (TLS or STARTTLS except to loopback, `https://` only), `connect_imap`/`connect_smtp` (tokio-rustls with the platform verifier, or a certificate pinned by its SHA-256 fingerprint after the user trusted it; STARTTLS), `client_for(Service)` (reqwest) and `client_for_events` (long-lived streams), the OAuth redirect listener, SRV and MX lookups, proxy profiles (none, system, manual HTTP CONNECT or SOCKS5 with exceptions; routes per service group `mail`, `sign_in`, `setup`, `updates`; IMAP and SMTP take only a SOCKS proxy from the system), and `Problem`, the classified connection error (DNS, refused, timeout, offline, TLS, certificate, sign-in, app password, admin consent, SMTP AUTH off, proxy). A test checks that no other code opens sockets, resolvers or HTTP clients | done |
| `imap` | `ImapClient` trait (capabilities, LIST, SELECT with QRESYNC, header/flag/message FETCH with CHANGEDSINCE, BODYSTRUCTURE and single parts, IDLE, STORE, COPY, MOVE, EXPUNGE, APPEND, SEARCH, CREATE/RENAME/DELETE), `imap::client` (the client on the wire: `imap-proto` parser, LITERAL+, AUTHENTICATE PLAIN or LOGIN, ENABLE QRESYNC), modified UTF-7 folder names, folder roles, and `imap::fake::FakeImap`, an in-memory server with UIDVALIDITY, MODSEQ and vanished UIDs | done |
| `sync` | Per-folder strategy (QRESYNC, CONDSTORE, plain, full resync on UIDVALIDITY change), `sync_mailbox`, `sync_folders`, bodies on demand (whole up to 2 MB, else text, HTML and inline pictures part by part), bodies of the newest 60 messages per folder ahead of time, attachments on demand, server SEARCH, IDLE wait; `SyncMachine`, the I/O-free connection state machine (connect, sync, idle or poll, backoff, sign-in needed, offline) | done |
| `engine` | One worker thread per account (a current-thread runtime with its own store connection): runs the `SyncMachine`, replays the outbox before each round, answers body, attachment and search requests from the UI, and reports `Notice`s (status, changed folders, new mail, outbox, sent); connectors for IMAP/SMTP with a password or OAuth (`NetConnector`) and JMAP (`JmapConnector`); `Command::Reconnect` after sleep or a network change | done |
| `actions` | What the UI does, local first: flags, move, archive, spam, delete, folders, send and drafts become outbox steps after a 5 second undo window; `undo` takes them back; `message_view` builds the reading pane's body (sanitized, quote folded, plain text marked) | done |
| `smtp` | Sending with lettre over a `network` stream (submission with STARTTLS or implicit TLS, AUTH PLAIN/LOGIN) and the connection check | done |
| `compose` | Messages built with mail-builder (text and HTML alternative, attachments, reply headers), the plain alternative from the editor's HTML, address parsing | done |
| `autoconfig` | Finding the servers of an address: presets, the provider's autoconfig file, the Thunderbird ISPDB, SRV records, presets by MX, a guess | done |
| `store` | SQLite (WAL, foreign keys), positional migrations through `PRAGMA user_version` (a newer database is refused), accounts, folders with sync state, messages, bodies, attachments, settings | done for M1 |
| `search` | FTS5 query from plain input (prefix words, phrases, `from:`/`subject:`; FTS syntax never passed through), newest first | done |
| `thread` | Conversations from Message-ID, In-Reply-To and References; adopts early replies, merges threads; replies without those headers join by base subject (a reply prefix, 45 days back, setting `subject_threads`) | done |
| `parse` | RFC 5322/MIME with mail-parser: addresses, dates, ids, text and HTML bodies, attachments, preview line | done |
| `sanitize` | ammonia allow-list: no scripts, forms, frames, style sheets or `url()`; remote images removed and counted unless allowed; links get `noopener noreferrer` | done |
| `outbox` | Offline queue: send (and the copy for Sent), flags, move, delete, drafts, folders; strict order per account, due times for the undo window, backoff, parked steps the server refused | done |
| `settings`, `settings_migrate` | One JSON object with a `version`; ordered, idempotent steps on the raw JSON, unknown keys kept (Arcalo's scheme) | done |
| `i18n` | `tr!`/`trf!` pairs for backend texts in German and English | done |

## Data model (`crates/mail-core/migrations`)

`0001_init.sql` below; `0003_sync_sources.sql` adds `messages.base_subject` and the JMAP mapping (`jmap_state`,
`jmap_mailboxes`, `jmap_emails`: a UID per message and folder, MODSEQs, „gone“ rows for QRESYNC's vanished UIDs);
`0002_mail_actions.sql` adds `messages.hidden` (moved or deleted here, not yet on the
server) and `reply_to_json`, `message_bodies.raw` (small messages kept whole, so their attachments open offline),
`attachments.section` and `data`, `accounts.signature`, `contacts` (recipient suggestions) and `remote_senders`
(senders whose images load).

| Table | Purpose |
|---|---|
| `accounts` | Provider, display name, address, sender name, `transport` (JSON of `account::Transport`). No secrets |
| `mailboxes` | Folders per account: name, delimiter, role, selectable, and the sync state `uid_validity`, `uid_next`, `highest_modseq`, `last_sync` |
| `messages` | One row per message per folder (`UNIQUE (mailbox_id, uid)`): ids for threading, subject, sender, recipients (JSON), dates, size, flags, keywords, `thread_id`, snippet, `has_body` |
| `message_bodies` | Text and the HTML as sent (sanitized each time it is shown, so a better sanitizer covers old mail), `remote_allowed` |
| `attachments` | Per message: part index, file name, type, size, Content-ID, inline, `local_path` once saved |
| `outbox` | The offline queue (kind, JSON payload, state, attempts, next try, last error) |
| `messages_fts` | FTS5, contentless with delete support, over subject, people and body text (unicode61, diacritics removed; ß folded to ss by the store) |
| `settings` | Key/value; the app settings are the JSON under `app` |

## Sync

1. Connect (TLS or STARTTLS through `network`), authenticate (password or XOAUTH2 with a fresh access token),
   `CAPABILITY`, `ENABLE QRESYNC` when offered.
2. `LIST` (with SPECIAL-USE): folders are added, updated, removed.
3. Per folder, inbox first: `SELECT` with `(QRESYNC (uidvalidity modseq))` when known. A different UIDVALIDITY
   clears the folder and fetches it again. Otherwise changed flags and vanished UIDs (QRESYNC), or flags
   `CHANGEDSINCE` plus a UID list when the count says something was removed (CONDSTORE), or flags and UID list
   (plain). New mail: headers above the stored UIDNEXT, in batches of 500.
4. Bodies of the newest 60 messages per folder up to 256 KB right after the sync (first lines in the list, search
   over their text, offline reading); others when a message is opened.
5. The inbox connection waits in IDLE (renewed every 25 minutes); other folders are polled every 5 minutes or
   synced when IDLE ends. Failures back off from 5 s to 5 minutes; a refused login stops until the user signs in.
6. The offline queue replays in order before each sync; the local change is shown at once.

`SyncMachine` decides these steps and does no I/O itself, so every transition is unit-tested; the shell runs one
per account on a worker and carries out its actions.

## Showing mail safely

- The core sanitizes HTML with an allow-list (`sanitize.rs`) and removes remote images unless the user allowed
  them for the message or its sender. Style attributes keep layout and color properties only, nothing that loads
  (a color given with the `background` shorthand is kept as `background-color`). The UI shows how many images were
  blocked and offers „Bilder laden“ for the message and „Immer für …“ for the sender.
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

`src-tauri/src/secrets.rs`: the `keyring` crate on Windows (Credential Manager), macOS and iOS (Keychain) and Linux
(Secret Service). On a Linux desktop without a Secret Service (or with `ARCALO_MAIL_SECRET_STORE=file`, which the
e2e tests use) passwords go to `secrets.json` in the data folder with mode 0600, and Settings → Privacy says so.
Android gets a small Kotlin plugin for the Android Keystore in milestone 3, as Arcalo's companion
app has; until then nothing is stored there. Entry names: service `de.mousewerk.arcalo.mail`, account
`account-<id>-password` (also a JMAP API token), `account-<id>-oauth-refresh-token` (rotated refresh tokens are
written back at once) and `proxy-<profile>-password`. OAuth access tokens are kept in memory only.

## UI (`ui/`)

Vite + React + TypeScript. `src/styles/tokens.css` is Arcalo's token set (spacing, type, radii, control heights,
motion, layers, the neutral state tints) adapted for mail and extended by a contrast theme; `styles.test.ts`
enforces tokens-only colors, no accent on selected/current/focused states, no side bars and `:focus-visible` rings.
Three panes (`Sidebar`, `MessageList`, `ReadingPane`) and a compose sheet; below 720 px one pane at a time with
back navigation and 44 px touch targets. In a plain browser `lib/api.ts` serves sample data (`lib/demo.ts`), so the
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
  address the app opened), the Gmail guide and JMAP. `e2e/shots` writes the README screenshots from English sample
  mailboxes on a fixed clock (`ARCALO_MAIL_NOW`).
