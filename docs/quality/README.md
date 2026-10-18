# Quality passes

Plans and findings of the quality passes before each release, as Arcalo keeps them (its `docs/quality/`): what
was checked, what was found (with root cause and fix), and what was deliberately left open and why. The bar is
Arcalo's: Obsidian/Linear-level polish in every area, on every device and theme of the matrix below.

| Release | Files |
|---------|-------|
| 1.1.0 (the first release) | `q110/plan.md` (areas, method, bar, status, matrix, exit criterion), `q110/findings.md` (what the guards found) |

Manual device checklists: `docs/testing/` (Android, iOS, Windows, macOS).

## The process

1. **Guards first.** Every rule of `CLAUDE.md` that a machine can check is a test (the table below). A rule that
   is only text in Arcalo becomes a test here where that is feasible. A new rule comes with its test in the same
   commit; a guard never gets an exception without the reason next to it, and the exception lists check that
   every listed place still exists.
2. **One plan per release** (`q<version>/plan.md`): the areas, for each the method and the bar, and a status
   column that is kept current. Findings go to `q<version>/findings.md` (or one file per area): severity,
   repro, root cause, fix, test.
3. **Root causes, not retries.** A flaky e2e test is a bug in the app or the test; „flake“ is not a root cause.
   Never skip, weaken or narrow a test (`scripts/e2e-conventions.test.mjs` refuses `.skip`, `.only`, `todo`).
4. **Rounds.** A round is: every area of the plan checked against its bar, all guards and checks of `CLAUDE.md`
   green, the full e2e suite run 5 times in a row without a single failure (the `e2e soak` workflow), the
   Android emulator suite green, the manual checklists of the changed areas done. A round is clean when it found
   nothing to fix.
5. **Exit criterion:** 3 consecutive fully clean rounds. Only then the release notes file is added (with the
   owner's go), which starts the release.

## The `e2e soak` workflow

`.github/workflows/e2e-soak.yml` builds the app once and runs the whole desktop suite N times (input `rounds`,
default 5; weekly on Sundays on `main`). It fails on any single failed test, writes the failed tests of each
round to the job summary and uploads the screenshots, `FAIL-*.png` and TAP report of every failed round
(artifact `e2e-soak-failures`). Start it from Actions → e2e soak → Run workflow (the workflow has to be on
`main` for the button to show).

## Guards

Every guard Arcalo has (its `CLAUDE.md`, `docs/i18n.md`, `docs/quality/`, `docs/testing/`, CI, the UI and Rust
tests that check the sources, the e2e harness) and its counterpart here. Status: **ported** (new on branch
`qa`), **present** (Arcalo Mail had it), **planned** (applies, comes with a later step named in the plan), or
**n/a** (with the reason). „Fixed after `and`“: the guard failed on `main` in files the phone round rewrote; after
its merge the findings were fixed on `qa` (`q110/findings.md`). Every ported guard is green on `qa`.

### Design (CSS and components)

| Guard | Arcalo source | Arcalo Mail | Status |
|-------|---------------|-------------|--------|
| Colors only as tokens | `ui/src/styles/styles.test.ts` „literal colors“ (with a list of allowed places) | `ui/src/lib/styles.test.ts` „uses colors from tokens.css only“ (no exceptions at all) | present |
| Every token in dark, light and contrast; only tokens that exist | text in `CLAUDE.md` | `styles.test.ts` „defines every token…“, „uses only tokens that exist“ | present |
| Balanced braces, no merge markers in stylesheets | `styles.test.ts` „stylesheets“ | `styles.test.ts` „stylesheets“ | ported |
| Weights, layers, focus ring, pill radii from tokens | `styles.test.ts` „design tokens“ | `styles.test.ts` „design tokens“ | ported |
| Mono font without ligatures | `styles.test.ts` | `styles.test.ts` | ported, green; 11 rules fixed after `and` |
| Uppercase labels with `--ls-eyebrow` | `styles.test.ts` | `styles.test.ts` | ported |
| No accent border, outline or ring on selected, current, focused states | `styles.test.ts` „selection and focus look“ | `styles.test.ts` (same, plus `--unread`, `.open`, `.expanded`, `aria-expanded`, `data-selected`) | ported (a simpler form was present) |
| No glow or gradient border for a state | text in `CLAUDE.md` („glows or gradient borders“) | `styles.test.ts` „no glow or gradient frame marks a state“ | ported (enforced here, text in Arcalo) |
| No colored bar at the left side (borders, thick gray lines, `::before` bars, inset shadows, upright bars, bar grid columns) | `styles.test.ts` | `styles.test.ts` | ported (only colored `border-left` was present) |
| No colored ring (inset shadow) marks a state | `styles.test.ts` | `styles.test.ts` | ported |
| Focus ring only on `:focus-visible`, neutral | text in `CLAUDE.md`, neutral tokens test | `styles.test.ts` „shows focus rings on :focus-visible only“ (outline and box-shadow on plain `:focus`), „focus and selection tokens are neutral“ | present, extended to box-shadow; tokens test ported |
| No faded text (opacity on content) | `styles.test.ts` „no faded text“ | `styles.test.ts` | ported |
| Inline styles in components draw no side bars or accent frames | `styles.test.ts` | `styles.test.ts` | ported |
| Endless animations only of opacity and transform | `styles.test.ts` „endless animations“ | `styles.test.ts`, plus a `prefers-reduced-motion` block must exist | ported |
| Text contrast 4.5:1 (muted 4.5 / 4 on rows, focus ring 3:1, status on its tint) | `ui/src/lib/contrast.test.ts`, `themes.test.ts` | `ui/src/lib/contrast.test.ts` (dark, light, contrast; contrast theme text 7:1) | ported, green; one pair in the contrast theme fixed after `and` |
| Custom and built-in theme families | `themes.test.ts`, `backdrop.test.ts`, `eventlook.test.ts` | – | planned: Arcalo's themes and reading mode are 1.1 scope; the tests come with them |
| Today in the time sheet, Git setup choices | `styles.test.ts` (view-specific) | – | n/a: views Arcalo Mail does not have; the general rules cover the calendar's today |
| Live dots never pulse forever | `styles.test.ts` „never run on the live dots“ | covered by „loop only opacity and transform“ | present in substance |

### Text, language and accessibility

| Guard | Arcalo source | Arcalo Mail | Status |
|-------|---------------|-------------|--------|
| Translation check (keys, placeholders, plurals, `tr!`/`trf!` pairs, typography, „du“, glossary, anglicisms, lengths) | `ui/src/lib/translations.test.ts`, `docs/i18n.md`, `glossary.json` | `ui/src/lib/translations.test.ts`, `docs/i18n.md`, `ui/src/locales/glossary.json` | present |
| Every key used in the code exists | `i18n.test.ts` | `t(key: TKey)`: the type check refuses an unknown key | present (typecheck) |
| No hard-coded UI text | `i18n-strings.test.ts` | `ui/src/lib/i18n-strings.test.ts` (mail names and protocols in `NAMES`, the preview's sample mailbox listed) | ported, green |
| Error texts of the backend through `tr!` | `crates/arcalo-core/src/error.rs` `error_texts_are_translated` | `crates/mail-core/tests/guards.rs` `error_texts_are_translated` (core and shell) | ported, 17 places fixed |
| Key combinations written one way | `shortcut.test.ts` „one notation“ | `ui/src/lib/keynotation.test.ts` (also „Strg + K“ with spaces) | ported |
| Icon-only buttons have a name | `a11y-buttons.test.ts` | `ui/src/lib/a11y-buttons.test.ts` | ported, green |
| Menu buttons say so (`aria-haspopup`); no button role that hides it | `a11y-menus.test.ts` | `ui/src/lib/a11y-menus.test.ts` (Arcalo Mail's menu openers) | ported, green; 1 button fixed after `and` |
| Enter and Escape safe during IME composition | `ime-guard.test.ts`, `lib/ime.ts` | `ui/src/lib/ime-guard.test.ts`, `ui/src/lib/ime.ts` | ported, green; 22 handlers; `listing.ts` fixed fixed after `and` |
| No emojis anywhere | text in `CLAUDE.md` | `ui/src/lib/emoji.test.ts` (every text file of the repository) | ported (enforced here, text in Arcalo) |
| English UI without German leftovers (e2e) | `e2e/lib/english.js` `germanLeftovers` | – | planned: q110 area i18n |
| Old product name out of sight | `branding.test.ts` | – | n/a: Arcalo Mail was never renamed |

### Security and data

| Guard | Arcalo source | Arcalo Mail | Status |
|-------|---------------|-------------|--------|
| No inline `<style>`/`<script>` in `index.html`, CSP | `splash.test.ts` | `ui/src/lib/html.test.ts` | present |
| Every connection through `network` (`client_for`) | `network.rs` `no_client_is_built_outside_the_resolver` | `network.rs` `no_connection_is_opened_outside_network` (sockets too) | present |
| Mail HTML only in the sandboxed frame, never `allow-scripts`, frame CSP | text in `CLAUDE.md` | `ui/src/lib/mailhtml.test.ts` | ported (enforced here); one place fixed |
| Secrets never in the settings | text (secrets in the credential store) | `guards.rs` `no_secret_in_the_settings`; `account.rs` `no_secret_is_part_of_the_stored_account` | ported / present |
| Diagnostics without people or secrets | `diagnostics.rs` `the_bundle_holds_info_settings_and_logs_but_no_secrets` | `diag.rs` `the_report_has_servers_but_no_people` | present |
| Lock screen keeps only its commands | `security.rs` | `src-tauri/src/security.rs` | present |
| No insecure updater endpoint; update key not a placeholder | `installer.rs`, release check | `installer.rs` `no_updater_endpoint_is_insecure`, `update.rs` `the_updater_needs_a_real_key` | present |
| Migrations positional (next number, no gap) | text in `CLAUDE.md` | `guards.rs` `migrations_are_positional` (files numbered without a gap, each in `store::MIGRATIONS` at its place) | ported (enforced here) |
| Settings change needs a step | `settings_migrate.rs` chain tests | `settings_migrate.rs` tests (present) and `guards.rs` `every_settings_key_comes_with_a_step` | ported |
| Settings of every release load | `settings_migrate.rs` `fixtures_of_every_release_load_without_losing_a_value` | – | planned: after 1.1.0 there is a released shape to keep (fixture `v1.1.0`) |
| Settings schema documented | `schema_is_documented_and_current` (company policies) | – | n/a: no admin policies |
| Every name in the data folder known | `datadir.rs` | – | n/a: no data folder migration (Annalo to Arcalo) |
| Read commands on the reader connection | `lib.rs` `read_commands_do_not_take_the_writer` | – | n/a: commands share one connection behind a lock (`AppState::store`), there is no reader pool to choose; lock waits are measured in the q110 performance area |
| Desktop-only commands are those Android leaves out | `platform.rs` | – | planned: q110 phone area (checked against `and`'s command list) |

### CI, release and e2e

| Guard | Arcalo source | Arcalo Mail | Status |
|-------|---------------|-------------|--------|
| fmt, clippy `-D warnings`, core/shell/UI tests, typecheck | `ci.yml` | `ci.yml` checks job | present |
| Clippy of the system-specific code on macOS (and Windows) | `ci.yml` macOS job | `ci.yml` desktop job, Windows and macOS | ported |
| Core file tests on Windows | `ci.yml` „Core tests on Windows (backups, encryption, Git sync)“ | `ci.yml` „Core tests on Windows (encryption, files, store)“ | ported |
| Update installer in release mode | `ci.yml` Windows and macOS | `ci.yml` desktop job, Windows and macOS | ported |
| PowerShell scripts parse (Windows PowerShell 5.1) | `ci.yml` Windows | `ci.yml` desktop job, Windows | ported |
| No Visual C++ runtime DLLs | `scripts/check-windows-runtime.ps1` | same | present |
| MSIX build, install, start, WACK | `ci.yml` Windows | – | planned: Microsoft Store is 1.1 scope (q110 updates/stores) |
| Release version pick, APK signer, feed, version stamp | `scripts/*.test.mjs` | `scripts/*.test.mjs` | present |
| Release notes: „## Improved“, „## Fixed“, „## Checklist for Windows (manual)“ | text in `CLAUDE.md` | `scripts/release-notes.test.mjs` | ported (enforced here) |
| Commit rules (owner's authorship, no trailers) | text in `CLAUDE.md` | `scripts/commit-rules.mjs` (each push), its test | ported (enforced here) |
| Superseded CI runs cancelled | `ci.yml` concurrency | same | present |
| e2e: failure screenshot (`guarded`) | `e2e/lib/harness.js` | `e2e/lib/harness.js`; `scripts/e2e-conventions.test.mjs` checks every test file | present; check ported, green; 3 shot files fixed after `and` |
| e2e: `app.reload()` waits for the new document | `harness.js` `reload` | `harness.js` `reload`; the conventions check refuses `location.reload` and `browser.refresh` | ported, green; 44 `browser.refresh()` calls in tests and shot files fixed after `and` |
| e2e: time budgets for CI (`budget`) | `harness.js` | `harness.js` | ported |
| e2e: app only via the variable, kill only own PIDs, no skips | text in `CLAUDE.md` | `scripts/e2e-conventions.test.mjs` | ported (enforced here) |
| e2e: own Xvfb display, private D-Bus, opener shim | `harness.js` | `harness.js` (`opened()`) | present |
| e2e: layout audit (clipped text, overflow, overlaps) | `e2e/lib/layout-audit.js` | `e2e/lib/geometry.js` on branch `e2e2` | planned: with `e2e2` |
| e2e: suite N times without a failure | process (q112 F) | `.github/workflows/e2e-soak.yml` | ported (a workflow here) |
| e2e on Android | `e2e/mobile/` | branch `e2e2` (`e2e/android/`) | planned: hook into CI when `e2e2` is merged (q110 plan) |
| Screenshots: whole window or one whole dialog, looked at | text in `CLAUDE.md` | text in `CLAUDE.md`; review in the plan | present (manual) |
| Quality plans and findings per release | `docs/quality/` | `docs/quality/` | ported |
| Manual device checklists | `docs/testing/` | `docs/testing/` (Android, iOS, Windows, macOS) | ported |
