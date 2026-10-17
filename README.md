<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="96" alt="Arcalo Mail icon">
</p>

<h1 align="center">Arcalo Mail</h1>

<p align="center">
  <b>Your mail, on your computer.</b><br>
  A calm, local-first e-mail app for IMAP/SMTP, Microsoft 365, Outlook.com, Gmail and JMAP accounts, from the makers of
  <a href="https://arcalo.mousewerk.de">Arcalo</a>. No telemetry, remote images blocked until you load them.
</p>

<p align="center">
  <a href="https://github.com/MouseWerk/arcalo-mail/actions/workflows/ci.yml"><img src="https://github.com/MouseWerk/arcalo-mail/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/MouseWerk/arcalo-mail" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-6b5bd6" alt="Platforms">
  <img src="https://img.shields.io/badge/built%20with-Tauri%202%20%C2%B7%20Rust%20%C2%B7%20React-2f2f3a" alt="Tauri 2, Rust, React">
</p>

<p align="center">
  <img src="docs/screenshots/inbox-light.png" alt="Arcalo Mail: all inboxes of two accounts sorted into tabs, a dot per account, and a conversation of three messages" width="100%">
</p>

Arcalo Mail keeps a copy of your mailboxes on your computer and works on that copy: the list opens at once, search
answers while you type, and reading, archiving, flagging and writing go on without a connection. Changes reach the
server in the background, in order, as soon as it can be reached. The app talks to your mail provider and nothing
else (except a newsletter's unsubscribe address, when you click Unsubscribe).

It works with any IMAP/SMTP provider, signs in to Microsoft 365 and Outlook.com with Microsoft (no password in the
app), guides you to a Gmail app password, and speaks JMAP where the provider offers it (Fastmail and others).

It speaks English and German and follows your system's language unless you choose one.

## A quick tour

### Three panes, conversations, keys

Folders of all accounts on the left, the list in the middle, the message or the whole conversation on the right.
Unread messages carry a dot, conversations show how many messages they hold, attachments, answers and flags show
in the row. Everything has a key: `j`/`k` to move, `e` to archive, `#` to delete, `r`, `a` and `f` to answer, `s`
to flag, `h` to snooze, `p` to pin, `u` to undo, `c` to write and `/` to search. Archive, delete and move wait five seconds before they reach
the server, so `u` takes them back without a trace.

<p align="center">
  <img src="docs/screenshots/inbox-dark.png" alt="The same inbox in the dark theme" width="100%">
</p>

### An inbox that sorts itself

The inbox comes in tabs: People, Newsletters, Notifications, Receipts and Other, each with its unread count. The
app sorts on your device, from what the header says (mailing list and unsubscribe headers, `Precedence`,
`Auto-Submitted`, machine senders, receipts and orders in German and English); anyone you wrote to is a person.
Move a sender to another tab once and its mail goes there from then on. Pin what must stay on top. Newsletters come
as one row per sender and open as a stream of their issues, one below the other. Focus shows only people and
pinned messages, and only they notify you. With two accounts or more, "All inboxes" opens first, with a colored
dot for each account. Done (`e`) archives, and an empty inbox says so, calmly.

<p align="center">
  <img src="docs/screenshots/inbox-tabs.png" alt="The work inbox sorted into tabs: people with a pinned message on top, and the counts of newsletters, notifications and receipts" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/inbox-zero.png" alt="An empty inbox: all done, unread mail waiting in two other tabs and one snoozed message coming back later" width="100%">
</p>

### New senders and subscriptions

The Screener is off unless you switch it on, in the setup or in the settings. Then mail from people who never
wrote to you waits under "New senders" until you let the sender in or turn it away once; contacts and replies to
your messages always come through. Mail of a sender you turned away goes to a "Screened Out" folder on the server,
now and later; nothing is deleted. Subscriptions lists every sender you can unsubscribe from: one click where the
sender supports it (RFC 8058), otherwise the e-mail the list asks for, or its web page after you confirm.

<p align="center">
  <img src="docs/screenshots/screener.png" alt="New senders: first-time senders with Let in and Turn away, one of their messages open" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/subscriptions.png" alt="Subscriptions: newsletters with one-click unsubscribe, one of them unsubscribed" width="100%">
</p>

### Later, not now

Snooze a message until later today, tomorrow, the weekend, next week or a time you pick (`h`): it waits in a
"Snoozed" folder on the server, so other apps see it out of the inbox too, and comes back unread at the top. Write
now and send later: the message waits in the app and goes out at its time (on a computer that was off then, at the
app's next start; on a phone with the next background check). "Remind me if no answer" brings a message back when
nobody answered by then. "Read later" keeps a list of what you put aside.

<p align="center">
  <img src="docs/screenshots/snooze-menu.png" alt="The snooze menu: later today, tomorrow, next week, or a time to pick" width="100%">
</p>

### HTML mail without the tracking

HTML mail is cleaned before it is shown (no scripts, forms, frames or style sheets) and drawn in a sandboxed frame
with its own content security policy. Remote images are blocked, so senders do not learn when you read their mail;
one click loads them for this message, another for every message of that sender. Tracking pixels and link trackers
(click redirects, `utm_` and similar parameters) are removed from every message, also when images load, and the
message says how many; no request ever goes to them. Quoted text is folded away, and every HTML message can also be
read as plain text.

<p align="center">
  <img src="docs/screenshots/reading-html.png" alt="A newsletter's issues as one stream: remote images blocked, the tracking pixel removed" width="100%">
</p>

### Writing

New messages, replies with the quote below and forwards with the original attachments open in a sheet next to what
you are reading. Recipients complete from everyone you have written with or heard from; drop files on the sheet to
attach them. Rich text or plain text, a signature per account, drafts saved to the server's Drafts folder, and five
seconds to take a message back after pressing Send. Sent mail is filed in Sent, except where the provider does that
itself, as Gmail does.

<p align="center">
  <img src="docs/screenshots/compose.png" alt="Forwarding an invoice: the compose sheet with a recipient, the forwarded text and the PDF attachment" width="100%">
</p>

### Search

Search runs on the local copy while you type: subjects, people and the text of every fetched message, newest first,
with accents and umlauts matched either way. For older mail whose text is still only on the server, "Search on the
server" asks the server and adds what it finds.

<p align="center">
  <img src="docs/screenshots/search.png" alt="Search results for the word launch: three messages of one conversation" width="100%">
</p>

### Adding an account

Enter your name, address and password. Arcalo Mail looks up the servers itself: presets for common providers (GMX,
WEB.DE, T-Online, mailbox.org, Posteo, iCloud, IONOS, STRATO, Gmail, Fastmail, Yahoo and others), the provider's
own autoconfig file, the Thunderbird provider database, DNS records and JMAP at the provider's well-known address.
Then it signs in and says plainly what went wrong if something did: a wrong password, a provider that wants an app
password or IMAP switched on first, an unknown certificate (with its fingerprint to compare and trust), a server
that does not answer. Everything can also be set up by hand, IMAP/SMTP or JMAP.

<p align="center">
  <img src="docs/screenshots/setup.png" alt="The first start: adding an account while the servers found in the provider's autoconfig file are checked" width="100%">
</p>

### Microsoft 365 and Outlook.com

Microsoft accounts sign in with Microsoft: Arcalo Mail opens Microsoft's sign-in page in your browser, you sign in
there and allow access to your mail, and the app continues on its own. Your password never reaches the app; it
keeps a sign-in token in your system's credential store and uses IMAP and SMTP with OAuth (XOAUTH2). Addresses at
outlook.com, hotmail.com and live.com go there directly, Microsoft 365 domains are recognized by their mail
servers, and the button works for any other. When an organization has not approved the app yet, or has switched
off sending over SMTP for the mailbox, the app says so and what IT can do (details in
[docs/microsoft.md](docs/microsoft.md)).

<p align="center">
  <img src="docs/screenshots/setup-microsoft.png" alt="Signing in with Microsoft: the app waits while the sign-in runs in the browser" width="100%">
</p>

### Gmail

Until signing in with Google arrives, Gmail and Google Workspace work over IMAP with an app password. For a Gmail
address the setup shows how to make one (2-Step Verification on, then the app passwords of your Google account) and
opens the page for you. Labels show as folders; All Mail, Starred and Important are not downloaded twice, search
shows each message once, archiving takes a message out of the inbox, and Gmail files sent mail itself.

<p align="center">
  <img src="docs/screenshots/setup-gmail.png" alt="A Gmail address in the setup: the steps to create an app password and the button that opens Google's page" width="100%">
</p>

### JMAP

Where the provider offers JMAP (Fastmail, Stalwart, Cyrus and others), Arcalo Mail uses it instead of IMAP: one
connection over HTTPS, changes since the last visit in one request, new mail pushed the moment it arrives. It signs
in with the password (or app password) or an API token. A JMAP account behaves like any other: offline, search,
the outbox, undo.

### Settings, themes and small windows

Settings apply at once: accounts with their name, sender name, signature and servers; a light, dark, high-contrast
or the system's theme; conversations and preview lines; what is downloaded ahead; notifications for new mail;
remote images and the senders you trust; proxy profiles. `Ctrl+K` (`Cmd+K` on a Mac) opens the command palette:
every command and every folder, a few keys away. The layout holds up from 900 pixels wide.

<p align="center">
  <img src="docs/screenshots/settings.png" alt="Settings, appearance: theme, language, conversations and preview lines" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/network.png" alt="Settings, network: the standard profile uses the system's proxy; which profile each kind of connection takes" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/palette.png" alt="The command palette: commands and the folders of both accounts" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/contrast.png" alt="The high-contrast theme with a message and two image attachments" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/narrow.png" alt="The app at 900 pixels wide" width="62%">
</p>

### Phones and tablets

On a phone the folders, the list and the message take the screen one at a time; on a tablet the list and the
message sit side by side and the folders slide in. A swipe over a message archives it or marks it read (you choose
what each side does), pulling the list down checks for mail, and the message's actions sit at the bottom, in reach
of the thumb. Microsoft accounts sign in through the system's browser. New mail arrives while the app is open; in
the background the phone checks now and then (Android about every 15 minutes, iOS when the system allows) and
tells you, without a server of ours in between.

<p align="center">
  <img src="docs/screenshots/mobile/phone-list-light.png" alt="The inbox on a phone" width="30%">
  <img src="docs/screenshots/mobile/phone-swipe-dark.png" alt="A swipe to the left over a message shows Archive, in the dark theme" width="30%">
  <img src="docs/screenshots/mobile/phone-message-light.png" alt="A conversation on a phone with the actions at the bottom" width="30%">
</p>

<p align="center">
  <img src="docs/screenshots/mobile/tablet-message-light.png" alt="A tablet: the list and the conversation side by side" width="62%">
</p>

## Features

- **Accounts:** any IMAP/SMTP provider with TLS or STARTTLS; Microsoft 365 and Outlook.com with Microsoft sign-in
  (OAuth 2.0 with PKCE, XOAUTH2); Gmail and Google Workspace with an app password; JMAP with push. Several accounts
  side by side, each with its own connection.
- **Sync:** new mail through IMAP IDLE or JMAP push; only what changed since the last visit where the server supports
  QRESYNC, CONDSTORE or JMAP changes; folder roles from the server (Inbox, Drafts, Sent, Archive, Spam, Trash);
  reconnects at once after sleep or a network change, and with growing pauses while the network is gone.
- **Network:** proxy profiles as in Arcalo (none, the system's, or an HTTP or SOCKS5 proxy with exceptions), chosen
  per kind of connection: mail servers, sign-in, the setup's lookups.
- **Offline:** reading, searching and every action work without a connection; changes and messages wait in the
  Outbox and go out in order when the server is back. The newest messages of each folder (or all of the last 30
  days) are fetched ahead, large ones and attachments when you open them.
- **Conversations:** by the reply headers, and replies that lost them by their subject.
- **Sorted inbox:** tabs for people, newsletters, notifications, receipts and other, sorted on the device from the
  headers; senders moved to another tab stay there; newsletter bundles; focus; pin; the unified inbox of all accounts.
- **Screener and subscriptions:** first-time senders wait until let in (off by default); turned-away senders go to
  "Screened Out"; one-click unsubscribe (RFC 8058), by e-mail, or the list's page after you confirm.
- **Time tools:** snooze (synced through a "Snoozed" folder), send later, reminders when no answer came, read later,
  done and inbox zero.
- **Tracker protection:** tracking pixels and link trackers removed from every message, counted per message.
- **Folders:** create, rename and delete folders; move messages by menu or key; spam and archive with one key.
- **Notifications:** a desktop notification for new mail, the unread count on the app icon and in the tray, with
  quick actions there.
- **Diagnostics:** Settings → About exports a text file for a support request: versions, settings, servers and the
  recent connection log, without addresses, messages or passwords.
- **Keyboard and screen readers:** the whole app works with the keyboard, focus stays visible, controls are
  labeled, and there is a high-contrast theme.

## Privacy

- **Local-first.** Your mail is stored in one SQLite database in your user folder and searched there.
- **No telemetry.** The app connects only to the mail servers you set up, Microsoft's sign-in for Microsoft
  accounts, while you add an account to the lookups that find their addresses, and to a newsletter's unsubscribe
  address when you ask for it. No analytics, no crash upload, no server of ours in between. Sorting, the Screener
  and the time tools run on the device.
- **Remote content blocked.** Images from the internet load only when you say so, for one message or one sender.
  Tracking pixels never load, and links lose their trackers.
- **Unsubscribing** is the one request to a sender's web address, and only when you click "Unsubscribe": a single
  POST that says nothing about you but the unsubscribe itself (RFC 8058).
- **Passwords and sign-in tokens** are kept in your system's credential store (Windows Credential Manager, the
  macOS and iOS keychain, the Secret Service on Linux; on Android encrypted with a key of the Android Keystore), never
  in the database, the settings or a log. On a Linux desktop without a keyring they go to a file only your user can
  read, and the settings say so.

## Platforms

| Platform | Status |
|---|---|
| Windows 10/11 | builds in CI (unsigned installer) |
| macOS 11+ | builds in CI (ad hoc signed) |
| Linux (deb, AppImage) | builds in CI; the end-to-end tests run here |
| Android 7+ | builds in CI (APK, and the AAB for Google Play once the release key is set up) |
| iOS 14+, iPadOS | the Rust part builds in CI; TestFlight and the App Store once the Apple account is set up |

## Install

There is no release yet. Installers for Windows, macOS and Linux will be attached to the
[releases](https://github.com/MouseWerk/arcalo-mail/releases) with the first version; until then, build from source.

## Build from source

Requirements: Rust (the version in `rust-toolchain.toml` is picked automatically), Node.js 22, and the Tauri
prerequisites of your platform (<https://tauri.app/start/prerequisites/>): WebView2 on Windows, the Xcode command
line tools on macOS, `libwebkit2gtk-4.1-dev` and friends on Linux.

```
npm --prefix ui install
cargo install tauri-cli --version "^2" --locked

# Development: the app with the Vite dev server
cd src-tauri && cargo tauri dev

# Release build (installers in target/release/bundle)
cd src-tauri && cargo tauri build
```

Only the UI, with sample data in the browser: `npm --prefix ui run dev` and open <http://127.0.0.1:1420>.

Tests:

```
cargo test -p mail-core -p mail-testserver -p arcalo-mail
npm --prefix ui run typecheck && npm --prefix ui test

# End-to-end on Linux: the real app against an IMAP/SMTP test server on this computer
npm ci --prefix e2e && e2e/run.sh
```

`crates/mail-testserver` is a small IMAP, SMTP and JMAP server for tests with an OAuth identity provider (its
options are listed at the top of `src/main.rs`); no local test talks to a real mail server. In CI a conformance
job also runs the client against Dovecot and GreenMail containers
(`cargo test -p mail-testserver --test conformance -- --ignored`, see `tests/conformance.rs`).

A build signs in to Microsoft only with a client id: `ARCALO_MAIL_MS_CLIENT_ID=… cargo tauri build`
([docs/microsoft.md](docs/microsoft.md)). The screenshots above come from `cd e2e && npm run shots`,
with made-up people on reserved example domains.

More: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/plan.md](docs/plan.md), [docs/microsoft.md](docs/microsoft.md),
[docs/i18n.md](docs/i18n.md).

## License

MIT, see [LICENSE](LICENSE). Copyright MouseWerk, Maurice Kleindienst.
