<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="96" alt="Arcalo Mail icon">
</p>

<h1 align="center">Arcalo Mail</h1>

<p align="center">
  <b>Your mail, on your computer.</b><br>
  A calm, local-first e-mail app for IMAP and SMTP accounts, from the makers of
  <a href="https://arcalo.mousewerk.de">Arcalo</a>. No telemetry, remote images blocked until you load them.
</p>

<p align="center">
  <a href="https://github.com/MouseWerk/arcalo-mail/actions/workflows/ci.yml"><img src="https://github.com/MouseWerk/arcalo-mail/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/MouseWerk/arcalo-mail" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-6b5bd6" alt="Platforms">
  <img src="https://img.shields.io/badge/built%20with-Tauri%202%20%C2%B7%20Rust%20%C2%B7%20React-2f2f3a" alt="Tauri 2, Rust, React">
</p>

<p align="center">
  <img src="docs/screenshots/inbox-light.png" alt="Arcalo Mail: folders of two accounts, the inbox and a conversation of three messages" width="100%">
</p>

Arcalo Mail keeps a copy of your mailboxes on your computer and works on that copy: the list opens at once, search
answers while you type, and reading, archiving, flagging and writing go on without a connection. Changes reach the
server in the background, in order, as soon as it can be reached. The app talks to your mail provider and nothing
else.

It speaks English and German and follows your system's language unless you choose one.

## A quick tour

### Three panes, conversations, keys

Folders of all accounts on the left, the list in the middle, the message or the whole conversation on the right.
Unread messages carry a dot, conversations show how many messages they hold, attachments, answers and flags show
in the row. Everything has a key: `j`/`k` to move, `e` to archive, `#` to delete, `r`, `a` and `f` to answer, `s`
to flag, `u` to undo, `c` to write and `/` to search. Archive, delete and move wait five seconds before they reach
the server, so `u` takes them back without a trace.

<p align="center">
  <img src="docs/screenshots/inbox-dark.png" alt="The same inbox in the dark theme" width="100%">
</p>

### HTML mail without the tracking

HTML mail is cleaned before it is shown (no scripts, forms, frames or style sheets) and drawn in a sandboxed frame
with its own content security policy. Remote images are blocked, so senders do not learn when you read their mail;
one click loads them for this message, another for every message of that sender. Quoted text is folded away, and
every HTML message can also be read as plain text.

<p align="center">
  <img src="docs/screenshots/reading-html.png" alt="A newsletter with three blocked remote images and the buttons to load them" width="100%">
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
WEB.DE, T-Online, mailbox.org, Posteo, iCloud, IONOS, STRATO, Gmail, Yahoo and others), the provider's own
autoconfig file, the Thunderbird provider database and DNS records. Then it signs in to both servers and says
plainly what went wrong if something did: a wrong password, a provider that wants an app password or IMAP switched
on first, an unknown certificate (with its fingerprint to compare and trust), a server that does not answer.
Everything can also be set up by hand.

<p align="center">
  <img src="docs/screenshots/setup.png" alt="The first start: adding an account while the servers found in the provider's autoconfig file are checked" width="100%">
</p>

### Settings, themes and small windows

Settings apply at once: accounts with their name, sender name, signature and servers; a light, dark, high-contrast
or the system's theme; conversations and preview lines; notifications for new mail; remote images and the senders
you trust. The layout holds up from 900 pixels wide.

<p align="center">
  <img src="docs/screenshots/settings.png" alt="Settings, appearance: theme, language, conversations and preview lines" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/contrast.png" alt="The high-contrast theme with a message and two image attachments" width="100%">
</p>

<p align="center">
  <img src="docs/screenshots/narrow.png" alt="The app at 900 pixels wide" width="62%">
</p>

## Features

- **Accounts:** any IMAP/SMTP provider; TLS or STARTTLS, password sign-in; several accounts side by side, each with
  its own connection.
- **Sync:** new mail through IMAP IDLE; only what changed since the last visit where the server supports QRESYNC or
  CONDSTORE; folder roles from the server (Inbox, Drafts, Sent, Archive, Spam, Trash); reconnects with growing
  pauses while the network is gone.
- **Offline:** reading, searching and every action work without a connection; changes and messages wait in the
  Outbox and go out in order when the server is back. The newest messages of each folder are fetched ahead, large
  ones and attachments when you open them.
- **Folders:** create, rename and delete folders; move messages by menu or key; spam and archive with one key.
- **Notifications:** a desktop notification for new mail and the unread count on the app icon.
- **Keyboard and screen readers:** the whole app works with the keyboard, focus stays visible, controls are
  labeled, and there is a high-contrast theme.

## Privacy

- **Local-first.** Your mail is stored in one SQLite database in your user folder and searched there.
- **No telemetry.** The app connects only to the mail servers you set up and, while you add an account, to the
  lookups that find their addresses. No analytics, no crash upload, no server of ours in between.
- **Remote content blocked.** Images from the internet load only when you say so, for one message or one sender.
- **Passwords** are kept in your system's credential store (Windows Credential Manager, the macOS keychain, the
  Secret Service on Linux), never in the database, the settings or a log. On a Linux desktop without a keyring they
  go to a file only your user can read, and the settings say so.

## Platforms

| Platform | Status |
|---|---|
| Windows 10/11 | builds in CI (unsigned installer) |
| macOS 11+ | builds in CI (ad hoc signed) |
| Linux (deb, AppImage) | builds in CI; the end-to-end tests run here |
| Android, iOS | planned, see [docs/plan.md](docs/plan.md) |

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

`crates/mail-testserver` is a small IMAP and SMTP server for tests (its options are listed at the top of
`src/main.rs`); no test talks to a real mail server. The screenshots above come from `cd e2e && npm run shots`,
with made-up people on reserved example domains.

More: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/plan.md](docs/plan.md), [docs/i18n.md](docs/i18n.md).

## License

MIT, see [LICENSE](LICENSE). Copyright MouseWerk, Maurice Kleindienst.
