# Arcalo Mail

E-mail for Windows, macOS, Linux, Android and iOS from the makers of [Arcalo](https://arcalo.mousewerk.de):
local-first, private by default, with the same calm design.

- **Your accounts:** IMAP/SMTP, Microsoft 365 and Outlook.com, Gmail and Google Workspace, JMAP.
- **Local-first:** mail is stored and searched on your device (SQLite with full-text search); reading and
  searching work offline, changes made offline are sent when the connection is back.
- **Private:** no telemetry, no own server between you and your mail provider. Remote images are blocked until you
  load them, HTML mail is shown sanitized in a sandbox without scripts. Passwords and sign-in tokens stay in your
  system's credential store.
- **German and English**, light, dark and high-contrast themes.

## Status

**Early development.** This repository holds the skeleton: the core library (accounts, local store with search,
parsing, HTML sanitizing, the sync engine against a fake IMAP server, with tests), the app shell and a three-pane
UI that shows sample data in the browser. It does not connect to a real mail server yet. The roadmap is
[docs/plan.md](docs/plan.md).

## Platforms

| Platform | Status |
|---|---|
| Windows 10/11 | builds in CI (unsigned installer) |
| macOS 11+ | builds in CI (ad hoc signed) |
| Linux (deb, AppImage) | builds in CI |
| Android 7+ | builds in CI (unsigned APK) |
| iOS 14+ | Rust library compiles in CI; the app needs an Apple developer account (see the plan) |

## Building

Requirements: Rust (the version in `rust-toolchain.toml` is picked automatically), Node.js 22, and the Tauri
prerequisites of your platform (<https://tauri.app/start/prerequisites/>): WebView2 on Windows, Xcode command line
tools on macOS, `libwebkit2gtk-4.1-dev` and friends on Linux.

```
npm --prefix ui install
cargo install tauri-cli --version "^2" --locked

# Development: the app with the Vite dev server
cd src-tauri && cargo tauri dev

# Release build (installers in target/release/bundle)
cd src-tauri && cargo tauri build
```

Only the UI, with sample data in the browser: `npm --prefix ui run dev` and open <http://127.0.0.1:1420>.

Mobile (needs the Android SDK and NDK, or Xcode):

```
cd src-tauri
cargo tauri android init && cargo tauri android dev
cargo tauri ios init && cargo tauri ios dev
```

Tests:

```
cargo test -p mail-core
npm --prefix ui run typecheck && npm --prefix ui test
```

More: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/i18n.md](docs/i18n.md).

## License

MIT, see [LICENSE](LICENSE). Copyright MouseWerk, Maurice Kleindienst.
