# Architecture

## Layers

```
 ui/ (React, Vite)                ── invoke() ──▶  src-tauri (commands, app state, secrets, platform)
   three panes, compose sheet,                          │
   sandboxed mail frame                                 ▼
                                               crates/mail-core
      ┌──────────┬──────────┬──────────┬───────────┬──────────┬───────────┬──────────┐
      │ account  │ network  │ imap     │ sync      │ store    │ parse     │ outbox   │
      │ (model,  │ (policy, │ (trait,  │ (strategy,│ (SQLite, │ sanitize  │ (offline │
      │ presets) │ connect) │ fake)    │ machine)  │ FTS5)    │ thread    │ queue)   │
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
| `account` | Providers (IMAP, Microsoft, Google, JMAP), transports (IMAP+SMTP, Graph, JMAP), servers, auth (password, OAuth 2.0), presets, validation, credential store names | done for M1 |
| `network` | The only place that opens connections. Now: the policy (TLS or STARTTLS except to loopback, `https://` only) and the test that no other code opens sockets or HTTP clients. Next: `connect_imap`/`connect_smtp` (tokio-rustls, platform verifier), `client_for(Service)` (reqwest), proxy profiles from Arcalo | policy done, connectors M1/M2 |
| `imap` | `ImapClient` trait (capabilities, LIST, SELECT with QRESYNC, UID SEARCH, header/flag/message FETCH with CHANGEDSINCE, IDLE), answer types, folder roles (SPECIAL-USE and common names), and `imap::fake::FakeImap`, an in-memory server with UIDVALIDITY, MODSEQ and vanished UIDs | trait and fake done, real client M1 |
| `sync` | Per-folder strategy (QRESYNC, CONDSTORE, plain, full resync on UIDVALIDITY change), `sync_mailbox`, `sync_account`, bodies on demand, IDLE wait; `SyncMachine`, the I/O-free connection state machine (connect, sync, idle or poll, backoff, sign-in needed, offline) | engine tested against the fake, loop M1 |
| `store` | SQLite (WAL, foreign keys), positional migrations through `PRAGMA user_version` (a newer database is refused), accounts, folders with sync state, messages, bodies, attachments, settings | done for M1 |
| `search` | FTS5 query from plain input (prefix words, phrases, `from:`/`subject:`; FTS syntax never passed through), ranking with subject first | done |
| `thread` | Conversations from Message-ID, In-Reply-To and References; adopts early replies, merges threads | done (no subject grouping yet) |
| `parse` | RFC 5322/MIME with mail-parser: addresses, dates, ids, text and HTML bodies, attachments, preview line | done |
| `sanitize` | ammonia allow-list: no scripts, forms, frames, style sheets or `url()`; remote images removed and counted unless allowed; links get `noopener noreferrer` | done |
| `outbox` | Offline queue: send, flags, move, delete; strict order per account, backoff, errors kept | storage done, replay M1 |
| `settings`, `settings_migrate` | One JSON object with a `version`; ordered, idempotent steps on the raw JSON, unknown keys kept (Arcalo's scheme) | done |
| `i18n` | `tr!`/`trf!` pairs for backend texts in German and English | done |

## Data model (`crates/mail-core/migrations/0001_init.sql`)

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
4. Bodies when a message is opened; later also for recent mail ahead of time (offline reading).
5. The inbox connection waits in IDLE (renewed every 25 minutes); other folders are polled every 5 minutes or
   synced when IDLE ends. Failures back off from 5 s to 5 minutes; a refused login stops until the user signs in.
6. The offline queue replays in order before each sync; the local change is shown at once.

`SyncMachine` decides these steps and does no I/O itself, so every transition is unit-tested; the shell runs one
per account on a worker and carries out its actions.

## Showing mail safely

- The core sanitizes HTML with an allow-list (`sanitize.rs`) and removes remote images unless the user allowed
  them for the message. The UI shows how many were blocked and offers „Externe Inhalte laden“.
- The reading pane renders the result in an `<iframe sandbox="allow-same-origin">` (no `allow-scripts`, so nothing
  in the mail runs; same origin only lets the app catch link clicks) with `srcdoc` and a meta CSP of its own:
  `default-src 'none'; img-src data: cid:` (plus `https:` once allowed), inline styles only. The app's CSP applies
  on top. Links open in the browser after a click, never inside the frame.
- Milestone 1 replaces `srcdoc` by a custom URI scheme served by the shell, so `cid:` images resolve to stored
  attachments and the CSP is sent as a header per message.

## App CSP (`src-tauri/tauri.conf.json`)

`script-src 'self'` (no inline scripts, no eval), `connect-src` only the IPC, `object-src 'none'`,
`img-src` with `https:` only for remote images the user allowed inside the mail frame (the frame's own CSP blocks
them otherwise). `ui/src/lib/html.test.ts` checks it.

## Secrets

`src-tauri/src/secrets.rs`: the `keyring` crate on Windows (Credential Manager), macOS and iOS (Keychain) and Linux
(Secret Service). Android gets a small Kotlin plugin for the Android Keystore in milestone 3, as Arcalo's companion
app has; until then nothing is stored there. Entry names: service `de.mousewerk.arcalo.mail`, account
`account-<id>-password` or `account-<id>-oauth-refresh-token`. OAuth access tokens are kept in memory only.

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
| IMAP | `async-imap` 0.12 (on `imap-proto`) over `tokio-rustls` | async, IDLE, extensions; raw commands for QRESYNC where its API ends |
| SMTP | `lettre` 0.11 (tokio, rustls) | mature, XOAUTH2, STARTTLS |
| TLS | `rustls` with `aws_lc_rs` and `rustls-platform-verifier` | as Arcalo; system trust store on every platform |
| HTTP (OAuth, Graph, JMAP) | `reqwest` 0.13 with rustls | as Arcalo, through `network::client_for` |
| MIME parsing | `mail-parser` 0.11 | as Arcalo; zero-copy, robust with broken mail |
| MIME building | `mail-builder` (with lettre) | the same author as mail-parser |
| HTML sanitizing | `ammonia` 4 | allow-list on html5ever |
| Store | `rusqlite` 0.40 bundled (FTS5) | as Arcalo; `bundled-sqlcipher` once encryption at rest comes |
| Secrets | `keyring` 3 | as Arcalo |

All of them were checked on crates.io; only the ones in use are in `Cargo.toml` now.
