# Plan

Arcalo Mail is the next-generation e-mail app for Windows, macOS, Linux, Android and iOS, built on Arcalo's design
and system. This page is the roadmap: what each milestone delivers, what the owner has to do outside the code, and
the decisions that are still open. Versions stay 0.x until milestone 1 is done (see open decisions).

## Where we are (skeleton)

- Cargo workspace (`crates/mail-core`, `src-tauri`), toolchain 1.94.1, edition 2024, as Arcalo.
- Core with tests: account model and presets, network policy with the „one network module“ check, IMAP trait and
  in-memory fake server, store schema 0001 with FTS5, search, threads, parsing, HTML sanitizing, offline queue
  storage, settings with migrations, and the sync engine (QRESYNC/CONDSTORE/plain/full) plus the connection state
  machine, run against the fake.
- Shell: Tauri 2 app with mobile entry point, CSP, commands for accounts, folders, messages, search, settings;
  secrets in the OS credential store.
- UI: three panes and compose sheet, light/dark/contrast, German and English with the translation check, design
  rule tests; sample data in a plain browser.
- CI: format, clippy, tests, UI checks; desktop builds for Windows, macOS and Linux; unsigned Android APK; iOS Rust
  library (the app build runs once signing exists).

Not there yet: a real connection. Everything that needs one is milestone 1.

## Milestone 1 – desktop, one IMAP/SMTP account

Goal: daily use with one IMAP/SMTP account on Windows, macOS and Linux.

1. **Connections** in `network`: `connect_imap`/`connect_smtp` with tokio-rustls and the platform verifier, TLS and
   STARTTLS, timeouts; certificate errors shown with the fingerprint and an explicit „trust this server“ (stored
   per account, as Arcalo's trusted fingerprints); proxy profiles taken over from Arcalo.
2. **Account setup**: address and password; servers from Mozilla's autoconfig (ISPDB, `autoconfig.<domain>`), SRV
   records (RFC 6186) and the presets, then a test login; manual settings as the fallback. Password into the
   credential store, never into the database.
3. **Real IMAP client** (`async-imap`) implementing `ImapClient`: CAPABILITY, ENABLE QRESYNC, LIST with
   SPECIAL-USE, SELECT (QRESYNC), UID FETCH with CHANGEDSINCE and VANISHED, UID SEARCH, IDLE, STORE, UID MOVE (COPY +
   UID EXPUNGE without MOVE), APPEND. A conformance test suite runs against the fake and, in CI, against a
   Dovecot container (and Greenmail for servers without QRESYNC).
4. **Sync loop** in the shell: one `SyncMachine` per account on a worker, IDLE on the inbox, polling for the other
   folders, network change and sleep/wake handling, progress and errors in the status bar.
5. **Local store and search**: headers of all folders, bodies of the last 30 days ahead of time (setting), search as
   you type over the local index; „Auf dem Server suchen“ for older mail (UID SEARCH, results cached).
6. **Read, compose, reply, forward**: reply/reply all/forward with quoting (done in the UI), sending with lettre
   and mail-builder (text and HTML, attachments, `In-Reply-To`/`References`), the copy in Sent (APPEND unless the
   server files it: Gmail, Outlook), drafts saved locally and to the Drafts folder, signatures per account.
7. **Threads**: conversation view across folders (Inbox + Sent), collapsed older messages; subject-based grouping
   with JWZ's rules and a time window for mail without headers (optional, setting).
8. **Offline queue**: every action works offline, applies at once, and replays in order (send, flags, move,
   delete); failures stay visible with „Erneut versuchen“; undo for move and delete.
9. **Attachments**: list, open, save; drag in to attach; inline `cid:` images through the custom URI scheme.
10. **HTML mail**: the shell serves the sanitized mail through its own URI scheme with a per-message CSP header;
    „Externe Inhalte laden“ per message and „Immer für diesen Absender“; tracking pixels counted; dark mode for
    simple mails (text colors only) as an option.
11. **App basics from Arcalo**: settings window with undo, update feed (`latest.json`, signed), single instance,
    tray with unread count, notifications for new mail, command palette, keyboard shortcuts, diagnostics package
    without mail content, e2e tests (WebdriverIO + tauri-driver) against the fake server.

## Milestone 2 – OAuth for Microsoft and Google, JMAP

1. **OAuth 2.0 with PKCE** as a public client (no client secret in the app): the system browser with a loopback
   redirect on desktop (RFC 8252), the platform's auth session on mobile (custom scheme / App Link). Refresh token
   in the credential store, access token in memory, silent refresh, „Anmeldung erforderlich“ when it fails.
2. **Microsoft 365 / Outlook.com**: IMAP and SMTP with XOAUTH2 first (one sync engine for everything), scopes
   `https://outlook.office.com/IMAP.AccessAsUser.All`, `SMTP.Send`, `offline_access`. Microsoft Graph as the second
   transport where IMAP/SMTP is switched off in the tenant (many tenants disable SMTP AUTH): `/me/messages` delta
   sync and `sendMail`.
3. **Gmail / Google Workspace**: IMAP and SMTP with XOAUTH2 (scope `https://mail.google.com/`), Gmail's labels
   mapped to folders (`X-GM-LABELS`, `X-GM-THRID` for threads, `[Gmail]/All Mail` not synced twice).
4. **JMAP** (RFC 8620/8621): session discovery, `Email/get`, `Email/changes`, `Mailbox/changes`, EventSource push,
   `EmailSubmission`; a second implementation of the sync interface (Fastmail and Stalwart for testing).

**Owner-side prerequisites (before milestone 2 can ship):**

- **Microsoft**: an app registration in Microsoft Entra ID (free; any Azure tenant): multi-tenant and personal
  Microsoft accounts, public client with loopback and mobile redirect URIs, delegated permissions IMAP.AccessAsUser.All,
  SMTP.Send, offline_access (and Mail.ReadWrite, Mail.Send for Graph). **Publisher verification** (needs a free
  Microsoft Partner Network ID tied to a verified domain, e.g. mousewerk.de): without it users see „nicht
  verifiziert“ and many company tenants block the consent. Tenant admins may still have to approve the app.
- **Google**: a Google Cloud project with an OAuth client (Desktop app type, plus Android and iOS clients), the
  consent screen with privacy policy and homepage on a verified domain. The full mail scope
  `https://mail.google.com/` is **restricted**: public use needs Google's verification and a yearly **CASA**
  security assessment by an authorized lab (a cost every year, scope depends on the tier). Until then the app runs
  in testing mode for at most 100 named test users, whose tokens expire after 7 days.
- **JMAP**: nothing to register.

## Milestone 3 – Android and iOS

1. Mobile UI: the narrow layout (one pane at a time, already in the skeleton) with gestures (swipe to archive or
   delete), share sheet for attachments, the system's dark mode and font size.
2. Secrets: Android Keystore through a Kotlin plugin (as Arcalo's companion app), iOS Keychain (keyring).
3. **New mail on a phone without an own server**: the app syncs with IMAP IDLE while it is in the foreground. In the
   background the platforms do not let an app keep a connection open: Android allows periodic work (WorkManager,
   at best every 15 minutes, delayed by Doze) or a foreground service with a permanent notification (an option for
   people who want it); iOS gives background app refresh at times the system chooses (often hours apart) and no
   long-lived sockets. Real push needs a server that holds IDLE or a provider webhook and sends via FCM/APNs; that
   contradicts „no own server“ and is an open decision. JMAP push and Microsoft Graph change notifications also
   need a reachable endpoint.
4. Builds: unsigned APK in CI now; a release keystore (owner, kept out of the repository) and the update feed for
   the APK as in Arcalo; iOS through TestFlight and the App Store once the Apple account exists.

## Later

- Unified inbox across accounts, smart folders (unread, flagged, today).
- Rules (local, run on new mail; Sieve upload where the server has ManageSieve).
- Snooze (local, with a hidden folder on the server so other clients see it) and send later (local queue; the app
  must be running, or the server's FUTURERELEASE where offered).
- PGP (Autocrypt, rPGP or Sequoia) and S/MIME (certificates from the system store), signing and encryption.
- Encryption at rest (SQLCipher, as Arcalo; off by default) and app lock.
- **Arcalo integration**: „An Arcalo senden“ (a mail becomes a page or is linked from one, as Arcalo's
  `arcalo-mail://` links), tasks from mails (a task with a link back), meeting notes from invitations; over a
  local, user-approved channel, no cloud.
- Calendar invitations (iMIP): accept or decline, add to Arcalo's calendar.
- Microsoft Store (MSIX) and winget for Windows, as Arcalo.

## Open owner decisions

1. **iOS distribution vs. „no paid certificates“.** Any iOS app on real devices outside a 7-day personal build
   needs the Apple Developer Program (99 USD per year): TestFlight, the App Store and the signing team for CI. Choose:
   pay for it (iOS ships), or keep iOS as „builds from source“ only (CI checks the Rust library), or drop iOS.
2. **Android distribution**: APK on GitHub releases only (free, own update feed as Arcalo's companion), Google Play
   (one-time 25 USD, Play's review and the target SDK rules), F-Droid (free, reproducible builds), or a mix.
3. **Google restricted scope**: pay for verification and the yearly CASA assessment, or offer Gmail with app
   passwords over IMAP (needs 2-step verification; Workspace admins may forbid it) and OAuth only in testing mode,
   or let advanced users bring their own OAuth client ID.
4. **Microsoft**: who owns the Entra app registration and the Partner Network ID (MouseWerk's domain), and whether
   Graph is supported from the start of milestone 2 or only when IMAP/SMTP is switched off for a user.
5. **Push on phones**: accept the platform limits (foreground IDLE, periodic checks, optional foreground service
   on Android), or run a small relay server (contradicts local-first and no own server today).
6. **Versioning**: 0.x until milestone 1, then 1.0 (Arcalo is 1.x); and whether Arcalo Mail releases with Arcalo or
   on its own.
7. **Website**: a section on arcalo.mousewerk.de or an own page/domain; when it is mentioned at all.
8. **Encryption at rest**: off by default as in Arcalo, or on by default for mail (more sensitive than notes).
9. **App icon and brand**: the icons are Arcalo's for now; a mail variant is needed before the first release.
10. **Windows signing**: unsigned NSIS plus the Microsoft Store (MSIX, signed by the Store) as Arcalo, and winget.

## Owner to-dos outside the code

- GitHub: the repository exists (MouseWerk/arcalo-mail, public); set the `AUTO_RELEASE` variable and branch
  protection when releases start; add the update signing key as a secret then (never in the repository).
- Microsoft Entra app registration and publisher verification (milestone 2).
- Google Cloud project, OAuth clients, consent screen, verification and CASA (milestone 2, decision 3).
- Apple Developer Program (decision 1), then the `APPLE_DEVELOPMENT_TEAM` secret for the iOS CI job.
- Android release keystore (milestone 3), kept outside the repository.
- A privacy policy page (needed for Google and Microsoft consent screens and the stores).
