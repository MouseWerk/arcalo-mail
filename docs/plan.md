# Plan

Arcalo Mail is the next-generation e-mail app for Windows, macOS, Linux, Android and iOS, built on Arcalo's design
and system. This page is the roadmap: what each milestone delivers, what the owner has to do outside the code, and
the decisions that are still open. Versions stay 0.x until milestone 1 is done (see open decisions).

## Where we are (milestone 1, desktop IMAP/SMTP)

- Cargo workspace (`crates/mail-core`, `crates/mail-testserver`, `src-tauri`), toolchain 1.94.1, edition 2024.
- Real connections: TLS and STARTTLS through `network` with the platform verifier, a certificate trusted by its
  fingerprint, classified errors (wrong password, app password needed, certificate, timeout, offline).
- Account setup with presets (GMX, WEB.DE, T-Online, mailbox.org, Posteo, iCloud, IONOS, STRATO and more), the
  provider's autoconfig file, the Thunderbird ISPDB, SRV and MX lookups, a connection check, manual settings;
  passwords in the credential store (a 0600 file on Linux desktops without a keyring).
- Own IMAP client on `imap-proto`: QRESYNC/CONDSTORE/plain sync, UIDVALIDITY, SPECIAL-USE, IDLE (renewed every 25
  minutes), flags, MOVE (or COPY and EXPUNGE), APPEND, SEARCH, folders; one worker per account with reconnect
  backoff and the offline queue replayed before each round. Bodies of the newest 60 messages per folder ahead,
  larger ones and attachments on demand.
- Sending with lettre over the same streams, the copy in Sent unless the provider files it, drafts in Drafts,
  signatures, five seconds to undo a send; archive, delete, move and spam with a five second undo.
- UI: three panes with a virtualized list, conversations, multi-select, keys (j/k, e, #, r, a, f, c, /, u, s);
  reading pane with the sanitized HTML in a sandboxed frame, remote images per message or sender, folded quotes,
  attachments to preview, open or save; compose with recipient suggestions, rich or plain text, dropped files;
  local and server search; notifications and the unread badge; settings; first-run setup; de/en; light, dark,
  contrast.
- Tests: core unit tests against the fake server, protocol tests against the test server on loopback, UI tests,
  e2e (setup, reading, writing, search, offline) in CI on Linux, README screenshots from `e2e/shots`.

Milestone 1 leftovers done in milestone 2: proxy profiles, reconnecting after sleep and network changes, the
prefetch setting, subject threading, the tray icon, the command palette, the diagnostics export and the
conformance job against Dovecot and GreenMail. Still open: the signed update feed.

## Where we are (milestone 2, Microsoft, Gmail, JMAP)

- **Microsoft 365 / Outlook.com**: OAuth 2.0 with PKCE in the system browser and a loopback redirect on
  127.0.0.1, tenant `common`, IMAP and SMTP with XOAUTH2, the refresh token in the credential store and refreshed
  before expiry and after a refusal, clear errors for admin consent and SMTP AUTH switched off. The client id
  comes from `ARCALO_MAIL_MS_CLIENT_ID` at build time (a GitHub Actions variable); without it the setup says so
  and offers IMAP with an app password. Owner steps: [microsoft.md](microsoft.md). Microsoft Graph as a second
  transport is not built: IMAP and SMTP cover Outlook.com and Microsoft 365 where SMTP AUTH is allowed.
- **Gmail / Google Workspace**: IMAP with an app password (owner decision), detected by domain or MX, with an
  in-app guide; All Mail, Starred and Important are not fetched ahead and searched once, archiving goes to All
  Mail, Gmail files sent mail itself.
- **JMAP**: discovery through `/.well-known/jmap` and the Fastmail preset, Basic or Bearer, `Mailbox/get` and
  `changes`, `Email/query`, `get` and `changes`, `Email/set`, `Email/import`, `EmailSubmission/set`, push over
  EventSource or polling, on the same store and sync engine (`jmap::JmapSession` implements `ImapClient`).
- **Mobile sign-in (milestone 3)**: the redirect `de.mousewerk.arcalo.mail://oauth` is registered in the Entra app
  and `oauth.rs` takes it; the platform browser session (Custom Tabs, `ASWebAuthenticationSession`) and the
  scheme handler are milestone 3 work.
- Tests: the fake identity provider and JMAP server in the core and the test server, XOAUTH2 over the wire,
  e2e for the Microsoft sign-in, the Gmail guide, JMAP, folders and bulk actions; conformance in CI.

## Where we are (milestone 3, Android and iOS)

- **UI**: the phone layout (folders, list, message one at a time; Android's Back walks back), the tablet layout
  (list and message side by side, folders in a drawer), swipes over rows (archive, delete, read, flag; per side in
  Settings), pull to refresh, the message's actions at the bottom, compose over the whole screen above the keyboard,
  44 px targets, safe areas, the system's dark mode and text size. Checked in the desktop app's touch layout
  (`e2e/tests/mobile.test.js`, pictures in `docs/screenshots/mobile`).
- **Secrets**: the Android Keystore (a key that never leaves it seals each secret) and the iOS Keychain (readable
  after the first unlock, for the background check).
- **Microsoft sign-in on phones**: Custom Tabs and `ASWebAuthenticationSession` with the redirect
  `de.mousewerk.arcalo.mail://oauth`.
- **New mail in the background, without a server**: WorkManager about every 15 minutes (Android) and background app
  refresh (iOS) run one check of every account; local notifications per message; the unread count as the launcher
  badge (the notification's number) and the iOS icon badge. IMAP IDLE and JMAP push while the app is in front.
- **Builds**: the Android project is committed; CI builds a signed APK and AAB once the keystore secrets exist and an
  iOS IPA (TestFlight for a tag) once the Apple secrets exist. Owner steps: [release-mobile.md](release-mobile.md).
- Open: a mail-specific app icon (Arcalo's icon is the placeholder), store screenshots at the stores' sizes, the
  share sheet for attachments and opening attachments in other apps on the phones, the in-app update check for the
  GitHub APK (Google Play updates the Play install), a Play service account for uploads from CI. To verify on
  devices: everything native (see release-mobile.md, "What only a device or CI can show").

## Where we are (1.0.0 features: the sorted inbox and the time tools)

- **Inbox that sorts itself**: tabs (people, newsletters, notifications, receipts, other) classified on the device
  from the header, a sender moved to another tab stays there, counts per tab, newsletter bundles opened as a
  stream, focus (people and pinned, notifications only for them), the unified inbox with account dots (first with
  two accounts or more), done and a calm inbox zero.
- **Screener** (off by default, offered in the setup and the settings): first-time senders wait in „Neue
  Absender“; contacts, replies and mail from before it was on pass; turned-away senders' mail moves to „Screened
  Out“, nothing is deleted.
- **Unsubscribe**: the subscriptions list; one-click POST (RFC 8058) through the network module, else the e-mail
  the list asks for, else its page after confirmation.
- **Tracker protection**: pixels and link trackers removed from every message with a count, never requested.
- **Time tools**: snooze (a „Snoozed“ folder on the server, local for messages without Message-ID), send later
  (desktop while the app runs and at its next start, phones in the background check), follow-up reminders on send
  and on a message, pin, read later.
- Open: snooze by keyword for servers that refuse new folders; IMAP keywords for pins so other devices see them.

## Where we are (feature round 2: assistant, rules, templates, contacts, POP3, attachments)

- **AI assistant** (off until a provider is set up): providers on this computer (Ollama, LM Studio, any
  OpenAI-compatible server) and cloud providers the user adds (OpenAI, Anthropic, Mistral, Azure OpenAI); a
  provider per account, a cloud provider never by default; the privacy line under every action. Summary (button,
  and by itself for long conversations), reply drafts in the user's tone learned from Sent, writing help in the
  compose sheet (shorter, friendlier, formal, spelling and grammar), translation, tasks and dates (a reminder or an
  .ics entry), „Postfach fragen“ (local FTS retrieval, answers with links), „Wichtig“ (heuristics, then the model).
  Streaming, stop, readable errors, answers kept per message. Tested against a fake OpenAI-compatible server.
- **Rules**: in the app (run on new mail, preview, run now on a folder) or on the server: Sieve over ManageSieve
  (merged into the active script between markers) and Microsoft inbox rules over Graph (`MailboxSettings.ReadWrite`,
  asked for at sign-in, docs/microsoft.md).
- **Templates and snippets** with placeholders and `;;` shortcuts; the signature never twice.
- **Contacts**: CardDAV address books (presets for iCloud, Fastmail, mailbox.org, Posteo, Nextcloud) and Microsoft
  contacts (`Contacts.Read`); address completion; contact card; pictures from the address book, BIMI (once per
  domain, SVG Tiny PS checked) or Gravatar (off by default); initials otherwise.
- **POP3** accounts (TLS or STLS, UIDL, leave on the server, delete after N days) with local folders and SMTP; set
  up by hand or with presets.
- **Attachments**: all files in one list, filtered by kind, sender, time and name, with preview, save, open and the
  way to the message.
- Open: BIMI's Verified Mark Certificates are not checked (the record, DMARC enforcement and the SVG profile are);
  the ManageSieve server is looked for on the IMAP host's port 4190 (another one can be entered in the rule editor;
  no discovery through DNS SRV `_sieve._tcp` yet); Graph rules for Microsoft accounts and Microsoft contacts are tested against a
  fake Graph only; a thumbnail column for pictures in „Anhänge“. (The assistant's dates now go to the app's own
  calendar, round 3.)

## Where we are (feature round 3: calendar, meetings, encryption, Arcalo)

- **Calendar** in Arcalo's design (its Kalender view ported: day, work week, week, month, list; time grid, all-day
  row, now line, side panel, quiet waiting and conflict states): create by dragging, double click or N; move,
  resize, rename and delete with „Rückgängig“; a meeting the user organizes changes only after „Aktualisierung an N
  Teilnehmende senden?“. Sources: CalDAV (presets for iCloud, Fastmail, mailbox.org, Posteo, Nextcloud; discovery,
  sync token or ctag, ETags, series, exceptions and time zones), Microsoft Graph (`Calendars.ReadWrite`, delta,
  change keys), JMAP Calendars, ICS subscriptions (read only), and a calendar on this device. Colors, hidden
  calendars, the default calendar, the work week; an offline queue with „Server-Version übernehmen“ / „Meine
  Änderung behalten“.
- **Meetings**: the invitation card (time, place, attendees and answers, conflicts, „Zusagen / Mit Vorbehalt /
  Absagen“ as an iTIP REPLY over SMTP or Graph's accept/decline; a CalDAV server that schedules answers itself),
  updates and cancellations applied, „Beitreten“ for Teams, Zoom, Webex, Google Meet and Jitsi in mail and
  calendar, the assistant's dates as a draft in the calendar's editor.
- **Encryption**: OpenPGP (keys made or imported, sign, encrypt, decrypt, verify, Autocrypt, Web Key Directory) and
  S/MIME (a .p12 in the credential store, chains against the system's roots, RSA); lock and check badges; no remote
  content in decrypted mail; files inside an encrypted message decrypted only to be saved. Interoperability checked
  by hand with GnuPG and OpenSSL in both directions.
- **„An Arcalo senden“**: a message as a page or a task through Arcalo's command line (`arcalo pages create … --stdin`,
  `arcalo tasks add …`) with Markdown and a `mid:` link back, else a Markdown file to save.
- **Notifications and widget**: Archive, Mark read and Reply on new-mail notifications (Linux actions, Windows toast
  buttons, Android actions, an iOS category); an Android home-screen widget with the unread count and the newest
  three mails of People.
- Open: drafts of encrypted messages are saved unencrypted on the server (as most clients do; encrypting drafts to
  the own key is next); the `mid:` link back needs Arcalo Mail registered as the handler of `mid:` addresses
  (desktop deep links); notification buttons on macOS (notify-rust has no actions there; the plain notification
  shows); iOS WidgetKit (the widget needs a Swift extension target in the Xcode project, planned for 1.1); JMAP
  Calendars has no per-event ETag, so a write there goes over what the server has (the next sync shows the
  result); S/MIME with RSA keys only (no ECDH key agreement yet); the Android and iOS parts (notification actions,
  the widget, the category delegate) are built in CI and need a check on devices (release-mobile.md); Graph
  calendars and invitations are tested against a fake Graph only.

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

## Milestone 2 – OAuth for Microsoft and Google, JMAP (done except Google sign-in and Graph)

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

- Smart folders (unread, flagged, today).
- Send later through the server's FUTURERELEASE where offered (the computer may be off then).
- Encryption at rest (SQLCipher, as Arcalo; off by default) and app lock.
- **Arcalo integration**, beyond „An Arcalo senden“ (round 3): meeting notes from invitations, Arcalo's
  `arcalo-mail://` links opening the message here.
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
- Apple Developer Program (decided: yes), then the iOS secrets of [release-mobile.md](release-mobile.md).
- Android release keystore and the Google Play account, kept outside the repository ([release-mobile.md](release-mobile.md)).
- A privacy policy page (needed for Google and Microsoft consent screens and the stores).
