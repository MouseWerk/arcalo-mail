# 1.1.0 quality: findings of the guards

What the guards of `docs/quality/README.md` found when they first ran on `main` (2dc93bb, 2026-10-17). Fixed
items say where; the items in files the phone and account switcher round (branch `and`) rewrote
were fixed after `and` was merged (below). The guard tests that were red on `main` until then are in the commit
„Guards that wait for the phone round“ on branch `qa`; the fixes follow it on the same branch.

## Fixed on `qa`

| # | Where | Rule | Fix |
|---|-------|------|-----|
| F1 | `crates/mail-core/src/engine.rs` (`NoConnection::refuse`) | error texts through `tr!` | „Keine Verbindung.“ / „No connection.“ instead of `offline` |
| F2 | `crates/mail-core/src/sevenbit.rs` (`address_header`, 2x) | error texts through `tr!` | „Die Adresse lässt sich nicht lesen.“ instead of `header` |
| F3 | `crates/mail-core/src/graph.rs` (creating a rule) | error texts through `tr!` | instead of `Microsoft Graph: no rule id` |
| F4 | `crates/mail-core/src/calendar/jmap.rs` (4x) | error texts through `tr!` | instead of `JMAP: no calendars`, `JMAP: no event`, `JMAP: no id` |
| F5 | `crates/mail-core/src/calendar/mod.rs` (5x) | error texts through `tr!` | instead of `appointment`, `graph`, `ics`, `key` |
| F6 | `src-tauri/src/commands.rs` (Microsoft sign-in) | error texts through `tr!` | instead of `no refresh token` |
| F7 | `src-tauri/src/crypto.rs` (3x) | error texts through `tr!` | instead of `message`, `attachment` |
| F8 | `ui/src/lib/listing.ts` (`keyAction`) | IME-safe Enter and Escape | asks `isComposing` (new `ui/src/lib/ime.ts`, from Arcalo); test in `lib.test.ts` |
| F9 | `ui/src/lib/compose.ts` (`editorHtmlToText`) | mail HTML only in the frame | parses in an inert `DOMParser` document instead of `innerHTML` on an element of the page (that one loads images) |

Four literals stay internal on purpose, listed with the reason in `INTERNAL` of
`crates/mail-core/tests/guards.rs` (calendar and address book sources always have their account).

## Fixed after the merge of `and` (main e2df834)

The phone round was merged into `main` on 2026-10-18; the guards then ran on the merged UI and every finding was
fixed on `qa` (line numbers as on e2df834).

### Mono font with ligatures (`styles.test.ts`, „sets the mono font without ligatures“)

Contextual alternates of the mono fonts (JetBrains Mono, Cascadia Code) draw `://` and `!==` as joined glyphs; a
fingerprint, a code or a server address must show each character. `font-variant-ligatures: none` added to:
`ui/src/styles/app.css:1261` `.fingerprint code`, `:1661` `.set-code`, `:2940` `.preview-text`, `:3276`
`.input-affix > .muted`, `:3298` `.snippet-trigger`, `:3699` `.protect-fingerprint`, `:3722` `.crypto-fp`,
`:3723` `.crypto-armor`, `:4473` `.sync-details` (new with `gm`); `ui/src/styles/base.css:384` `.dialog-detail`;
`ui/src/styles/security.css:124` `.lock-cmd`.

### Contrast (`contrast.test.ts`, „contrast“)

`ui/src/styles/tokens.css` (contrast theme): `--danger` `#ff9b9b` on `--bg-current` over `--bg-overlay` was
4.46:1, below 4.5:1 (an error text on the current row of a menu or a list in a dialog). Now `#ffa3a3` (4.72:1);
every other pair of the theme still passes.

### Menu buttons (`a11y-menus.test.ts`)

`ui/src/components/Sidebar.tsx:266` the folder's „more“ button (`folder-more`) opened a menu without
`aria-haspopup="menu"`: screen readers did not announce a menu. Added.

### Enter and Escape during an input-method composition (`ime-guard.test.ts`)

These handlers acted on Enter or Escape without asking `ui/src/lib/ime.ts`: with a Japanese, Chinese or Korean
input method, or a dead key on a Mac, the Enter that picks a candidate submitted the field, and the Escape that
cancels the composition closed the dialog. WebKit on macOS sends that key with `isComposing` false and keyCode
229, so `e.nativeEvent.isComposing` alone (as `CalendarView.tsx` had it) was not enough. They use
`isKey(e, "Enter")`, `isKey(e, "Escape")` and `isComposing(e)` now:
`ui/src/App.tsx:2488` (attachment preview), `:2591` (passphrase); `components/AskView.tsx:74`;
`components/Assistant.tsx:183`; `components/CalendarSettings.tsx:430`; `components/CalendarView.tsx:302`, `:850`,
`:1076`, `:1124`, `:1231`; `components/CommandPalette.tsx:68`; `components/ComposeSheet.tsx:198`, `:539`, `:650`;
`components/Meeting.tsx:205`; `components/MessageList.tsx:359`; `components/Security.tsx:388`;
`components/Settings.tsx:623`; `components/TimeMenus.tsx:128`; `components/ui.tsx:78` (Dialog), `:202`
(PromptDialog), `:328` (Menu). The macOS checklist (`docs/testing/macos.md`) has the manual check with a dead key
and the Japanese input method.

### e2e conventions (`scripts/e2e-conventions.test.mjs`)

- `app.browser.refresh()` instead of `app.reload()` in 44 places: every test file that switches a setting and
  reloads (`accounts`, `ai`, `attachment-safety`, `attachments`, `calendar`, `compose`, `contacts`, `crypto`,
  `folders`, `meetings`, `mobile`, `mobile-tools`, `offline`, `perf`, `phone-insets`, `read`, `rules`,
  `screener`, `search`, `security`, `sorting`, `templates`, `time`, `unified`, `updates`) and the shot scripts
  (`readme-shots`, `mobile-shots`, `store-shots`, `phone-tour`). `refresh()` returns before the old document
  goes; most places did not wait for `ready` at all, so the next click or key could reach the old page. This is
  the likely root cause of the one-off failure of `mobile-tools` „asks the mailbox on a phone“ and „edits a rule
  on a phone“ (`.folders-btn` not clickable after 8 s) in CI run 39068876509, which passed on the rerun:
  `mobile-tools.test.js:33` reloaded right before. All go through `app.reload()` now, which waits for the new
  document.
- `e2e/shots/mobile-shots.test.js`, `store-shots.test.js` and `phone-tour.test.js` did not wrap their tests in
  `guarded()`: a failed screenshot run left no `FAIL-*.png`. Wrapped.

## Passed on the first run

Stylesheet braces and merge markers, literal colors, weights, layers, focus ring and pill radii as tokens,
uppercase labels, accent frames on selected, current and focused states, glows and gradient frames, left bars
(borders, pseudo elements, inset shadows, bar columns), colored rings, faded text (one decoration, the calendar's
`faint-mark`, allowed as in Arcalo), inline styles in components, neutral focus and selection tokens, endless
animations, `:focus` rings outside `:focus-visible`, contrast of the dark and light themes, icon-only buttons
without a name, button roles, hard-coded UI text (the sample mailbox of the plain-browser preview is listed),
key notation, emojis, mail HTML outside the frame and `allow-scripts`, the release notes' shape, the commit
rules, and the e2e suite's `guarded`, skips, app path, kills and real servers.
