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
- The first public release is 1.1.0 (there is no 1.0.0); later releases are 1.x. Decisions and their dates:
  `docs/decisions.md`.
- Accounts in 1.1.0: IMAP/SMTP, POP3/SMTP (with "leave on server"), Microsoft 365/Outlook.com (OAuth), JMAP, and
  presets for iCloud, Yahoo, AOL, GMX, WEB.DE and T-Online. Gmail/Google Workspace works over IMAP with an app
  password (the setup says how to create one); Google sign-in (OAuth) comes later, after Google's verification.
  Exchange on premises (EWS) is part of the 1.1 scope. Remote content in mail is blocked by default.
- App language: German uses "du", plus English. Glossary: `ui/src/locales/glossary.json`, rules in `docs/i18n.md`.
- Encryption at rest is off by default (as in Arcalo); settings apply instantly with undo.
- Code signing certificates are off the table (no paid certificate), as in Arcalo; the Microsoft Store (MSIX) and
  winget are the Windows store paths. iOS is decided: MouseWerk joins the paid Apple Developer Program and Arcalo
  Mail ships through TestFlight and the App Store (the one exception, for the App Store only). Android: Google
  Play plus the APK attached to each GitHub release.
- No pull requests unless asked. No force push, branch deletion or history rewrite without an explicit request.
- Never commit signing keys (update signing, Android keystore). Secrets only in the OS credential store.

## Code rules

- Match the surrounding style and comment density. No emojis anywhere.
- No inline `<style>`/`<script>` in `ui/index.html` (CSP; `ui/src/lib/html.test.ts` checks it).
- Every connection goes through `mail_core::network`: IMAP and SMTP sockets and every HTTP client
  (`client_for(Service)`). A test checks it (`no_connection_is_opened_outside_network`).
- Plain-text connections only to loopback hosts; TLS or STARTTLS everywhere else (`network::check_server`).
- Passwords and OAuth tokens live in the OS credential store (`src-tauri/src/secrets.rs`, names from
  `account::secret_key`), never in the database, the settings or a log.
- HTML mail is sanitized in the core (`sanitize.rs`) and shown in a sandboxed iframe without scripts and with its
  own CSP (`ui/src/lib/mailframe.ts`). Never render mail HTML anywhere else, never add `allow-scripts`.
- UI texts go through the i18n catalogs (`ui/src/locales/de.ts`, `en.ts`; backend `tr!`/`trf!` pairs); the
  translation check `ui/src/lib/translations.test.ts` must pass.
- Use the design tokens in `ui/src/styles/tokens.css`; no one-off colors, weights, layers or radii
  (`ui/src/lib/styles.test.ts` checks them). Text reaches 4.5:1 in every theme (`contrast.test.ts`).
- No accent-colored borders, outlines, left bars, glows or gradient borders for selected, active or current
  states (the "AI look"); show them with a quiet neutral background tint and text weight or color. Keyboard
  focus stays visible, but only on `:focus-visible` and in a neutral style. No colored bars on the left side
  anywhere either (cards, toasts, notices, callouts, nav); use a tinted background and an icon. A plain
  blockquote may keep a thin neutral gray line. `ui/src/lib/styles.test.ts` fails on each of these. Quality bar:
  Obsidian/Linear-level polish in light, dark and contrast themes, 900–1920 px, split panes, and phones.
- Accessibility and input: icon-only buttons have an `aria-label` (`a11y-buttons.test.ts`), a button that opens a
  menu has `aria-haspopup` (`a11y-menus.test.ts`), and every handler of Enter or Escape asks `isComposing`/`isKey`
  of `ui/src/lib/ime.ts` first (`ime-guard.test.ts`). No hard-coded UI text (`i18n-strings.test.ts`); key
  combinations are written `Strg+K` (`keynotation.test.ts`). No emojis (`emoji.test.ts` checks the whole
  repository).
- Error texts that reach the UI go through `tr!`/`trf!` (`crates/mail-core/tests/guards.rs`, with the
  positional migrations, the settings steps and no secrets in the settings).
- Migrations in `crates/mail-core/migrations` are positional: take the next number, never fill a gap. Settings
  changes need a step in `crates/mail-core/src/settings_migrate.rs` (`SETTINGS_VERSION`).
- Release notes: `docs/releases/v<version>.md` with "## Improved", "## Fixed" and
  "## Checklist for Windows (manual)"; user-facing lines only.

## Checks

```
cargo fmt --all --check
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --workspace --all-targets -- -D warnings
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p mail-core -p mail-testserver -p arcalo-mail
npm --prefix ui run typecheck && npm --prefix ui test
```

The shell embeds the UI at compile time: run `npm --prefix ui run build` before building or linting
`src-tauri`. On a machine without WebKitGTK/WebView2, `cargo clippy -p mail-core --all-targets -- -D warnings` and
`cargo test -p mail-core` cover the core.

e2e tests (WebdriverIO + tauri-driver, as Arcalo's `e2e/`) run the real app against the IMAP/SMTP test server
(`crates/mail-testserver`, on loopback ports; never a real mail server). The app embeds the UI, so build first:

```
npm --prefix ui run build
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo build -p arcalo-mail --features custom-protocol -p mail-testserver
cd e2e && ARCALO_MAIL_APP="$PWD/../target/debug/arcalo-mail" node --test --test-concurrency=1 tests/<file>.test.js
```

`e2e/run.sh` does all of it. Pass the app only via `ARCALO_MAIL_APP`. The harness starts Xvfb on `DISPLAY` (default
`:76`) when nothing runs there. Never kill processes by name or pattern (other runs may share the machine); kill
only PIDs you started. Prefer root-cause fixes; never skip or weaken a test. Every test is wrapped in `guarded`
(a screenshot on failure), a reload goes through `app.reload()`; `scripts/e2e-conventions.test.mjs` checks both.
A test that fails once in five runs is a bug: the `e2e soak` workflow runs the whole suite N times.

## Quality

The quality process is Arcalo's: `docs/quality/README.md` (every guard, its Arcalo source and status; the device
and theme matrix; the exit criterion), one plan and findings file per release (`docs/quality/q110/` for 1.1.0), and
manual device checklists in `docs/testing/`.

README screenshots: `cd e2e && npm run shots` writes `docs/screenshots/` from English sample mailboxes
(`e2e/shots/mailbox.js`) on a fixed clock. Every picture shows the whole window or one whole dialog; look at each
one before committing it.

## Release

As Arcalo's, described in `docs/release.md`: a `docs/releases/v<version>.md` for a version newer than every tag
releases once CI is green on `main` (`auto-release.yml` → `release.yml`; the repository variable `AUTO_RELEASE=false`
switches it off). The desktop updater reads `latest.json` from the GitHub release, signed with the update key: the
public key in `plugins.updater.pubkey` of `src-tauri/tauri.conf.json` (compiled in as `ARCALO_MAIL_UPDATER_PUBKEY` by
the release only), the private key only in the repository secrets. The release refuses to start while the public key
is the placeholder. Add the notes file only with the owner's go, in the last push of a release. Store texts, the
privacy policy text and the store screenshots (`cd e2e && npm run shots:store`) are in `docs/store/`.
