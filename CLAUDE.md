# Working on Arcalo Mail

Arcalo Mail is a local-first e-mail app for Windows, macOS, Linux, Android and iOS, built on Arcalo's design and
system: one Tauri v2 codebase with a Rust workspace (`crates/mail-core`, `src-tauri`) and a React UI in `ui/`. Read
`docs/ARCHITECTURE.md`, `docs/plan.md` and `README.md` first. Arcalo itself is MouseWerk/Arcalo; when in doubt how
something is done, do it the way Arcalo does.

## Owner decisions (standing)

- Commits are authored as `Maurice Kleindienst <kleindiema@gmail.com>`. No co-author or session trailers, no
  assistant or model names in commits, code, comments or docs.
- Public repository, MIT license (MouseWerk, Maurice Kleindienst).
- Local-first, no telemetry: mail is stored and searched on the device; the app talks only to the user's mail
  servers, the OAuth endpoints of the chosen provider and the update feed. No own server, no analytics, no crash
  upload.
- Accounts in the first version: IMAP/SMTP, Microsoft 365/Outlook.com (OAuth), Gmail/Google Workspace (OAuth),
  JMAP. Remote content in mail is blocked by default.
- App language: German uses "du", plus English. Glossary: `ui/src/locales/glossary.json`, rules in `docs/i18n.md`.
- Encryption at rest is off by default (as in Arcalo); settings apply instantly with undo.
- Code signing certificates are off the table (no paid certificate). iOS distribution needs the paid Apple
  Developer Program: that is an open owner decision (`docs/plan.md`), not a default.
- No pull requests unless asked. No force push, branch deletion or history rewrite without an explicit request.
- Never commit signing keys (update signing, Android keystore). Secrets only in the OS credential store.

## Code rules

- Match the surrounding style and comment density. No emojis anywhere.
- No inline `<style>`/`<script>` in `ui/index.html` (CSP; `ui/src/lib/html.test.ts` checks it).
- Every connection goes through `mail_core::network`: IMAP and SMTP sockets and every HTTP client
  (`client_for(Service)` once HTTP arrives). A test checks it (`no_connection_is_opened_outside_network`).
- Plain-text connections only to loopback hosts; TLS or STARTTLS everywhere else (`network::check_server`).
- Passwords and OAuth tokens live in the OS credential store (`src-tauri/src/secrets.rs`, names from
  `account::secret_key`), never in the database, the settings or a log.
- HTML mail is sanitized in the core (`sanitize.rs`) and shown in a sandboxed iframe without scripts and with its
  own CSP (`ui/src/lib/mailframe.ts`). Never render mail HTML anywhere else, never add `allow-scripts`.
- UI texts go through the i18n catalogs (`ui/src/locales/de.ts`, `en.ts`; backend `tr!`/`trf!` pairs); the
  translation check `ui/src/lib/translations.test.ts` must pass.
- Use the design tokens in `ui/src/styles/tokens.css`; no one-off colors or sizes (`styles.test.ts` checks colors).
- No accent-colored borders, outlines, left bars, glows or gradient borders for selected, active or current
  states (the "AI look"); show them with a quiet neutral background tint and text weight or color. Keyboard
  focus stays visible, but only on `:focus-visible` and in a neutral style. No colored bars on the left side
  anywhere either (cards, toasts, notices, callouts, nav); use a tinted background and an icon. A plain
  blockquote may keep a thin neutral gray line. Quality bar: Obsidian/Linear-level polish in light, dark and
  contrast themes, 900–1920 px, split panes, and phones.
- Migrations in `crates/mail-core/migrations` are positional: take the next number, never fill a gap. Settings
  changes need a step in `crates/mail-core/src/settings_migrate.rs` (`SETTINGS_VERSION`).
- Release notes: `docs/releases/v<version>.md` with "## Improved", "## Fixed" and
  "## Checklist for Windows (manual)"; user-facing lines only.

## Checks

```
cargo fmt --all --check
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --workspace --all-targets -- -D warnings
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p mail-core -p arcalo-mail
npm --prefix ui run typecheck && npm --prefix ui test
```

The shell embeds the UI at compile time: run `npm --prefix ui run build` before building or linting
`src-tauri`. On a machine without WebKitGTK/WebView2, `cargo clippy -p mail-core --all-targets -- -D warnings` and
`cargo test -p mail-core` cover the core.

e2e tests (WebdriverIO + tauri-driver, as Arcalo's `e2e/`) arrive with milestone 1 and run against a fake IMAP
server. Pass the app only via an environment variable. Never kill processes by name or pattern (other runs may
share the machine); kill only PIDs you started. Prefer root-cause fixes; never skip or weaken a test.

## Release

Not set up yet. It will follow Arcalo (`docs/release/automatic.md` there): a `docs/releases/v<version>.md` for a
version newer than every tag releases once CI is green on `main`; the desktop updater reads `latest.json` from the
GitHub release, signed with the update key (`ARCALO_MAIL_UPDATER_PUBKEY` compiled in, the private key only in the
repository secrets). Add the notes file only with the owner's go.
