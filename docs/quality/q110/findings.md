# 1.1.0 quality: findings of the guards

What the guards of `docs/quality/README.md` found when they first ran on `main` (2dc93bb, 2026-10-17). Fixed
items say where; open items are in files the phone and account switcher round (branch `and`) rewrites, so they
are fixed after `and` is merged, on top of it. The guard tests that are red until then are in the commit
„Guards that wait for the phone round“ on branch `qa` (see the README); merge it after `and` together with the
fixes below.

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

## Open, in files of the `and` round (fix after it is merged)

### Mono font with ligatures (`styles.test.ts`, „sets the mono font without ligatures“)

Contextual alternates of the mono fonts (JetBrains Mono, Cascadia Code) draw `://` and `!==` as joined glyphs; a
fingerprint, a code or a server address must show each character. Add `font-variant-ligatures: none` to:

- `ui/src/styles/app.css:1245` `.fingerprint code`
- `ui/src/styles/app.css:1645` `.set-code`
- `ui/src/styles/app.css:2924` `.preview-text`
- `ui/src/styles/app.css:3260` `.input-affix > .muted`
- `ui/src/styles/app.css:3282` `.snippet-trigger`
- `ui/src/styles/app.css:3672` `.protect-fingerprint`
- `ui/src/styles/app.css:3695` `.crypto-fp`
- `ui/src/styles/app.css:3696` `.crypto-armor`
- `ui/src/styles/base.css:377` `.dialog-detail`
- `ui/src/styles/security.css:123` `.lock-cmd`

### Contrast (`contrast.test.ts`, „contrast“)

- `ui/src/styles/tokens.css:451` (contrast theme): `--danger` `#ff9b9b` on `--bg-current` over `--bg-overlay` is
  4.46:1, below 4.5:1 (an error text on the current row of a menu or a list in a dialog). `#ffa3a3` gives 4.72:1;
  check the other pairs of the theme again after the change.

### Menu buttons (`a11y-menus.test.ts`, „every button that opens a menu has aria-haspopup“)

- `ui/src/components/Sidebar.tsx:239` the folder's „more“ button (`folder-more`) opens a menu without
  `aria-haspopup="menu"`: screen readers do not announce a menu.

### Enter and Escape during an input-method composition (`ime-guard.test.ts`)

These handlers act on Enter or Escape without asking `isComposing(e)` or using `isKey(e, …)` of
`ui/src/lib/ime.ts`: with a Japanese, Chinese or Korean input method, or a dead key on a Mac, the Enter that picks
a candidate submits the field, and the Escape that cancels the composition closes the dialog. WebKit on macOS
sends that key with `isComposing` false and keyCode 229, so `e.nativeEvent.isComposing` alone (as in
`CalendarView.tsx`) is not enough.

- `ui/src/App.tsx:2267`, `ui/src/App.tsx:2370`
- `ui/src/components/AskView.tsx:73`
- `ui/src/components/Assistant.tsx:183`
- `ui/src/components/CalendarSettings.tsx:430`
- `ui/src/components/CalendarView.tsx:298`, `:845`, `:1071`, `:1119`, `:1226`
- `ui/src/components/CommandPalette.tsx:65`
- `ui/src/components/ComposeSheet.tsx:192`, `:526`, `:605`
- `ui/src/components/Meeting.tsx:205`
- `ui/src/components/MessageList.tsx:319`
- `ui/src/components/Security.tsx:383`
- `ui/src/components/Settings.tsx:621`
- `ui/src/components/TimeMenus.tsx:123`
- `ui/src/components/ui.tsx:45` (Dialog), `:169` (PromptDialog), `:284` (Menu)

Line numbers are those of `main` at 2dc93bb; after the merge of `and` the test prints the new ones.

### e2e conventions (`scripts/e2e-conventions.test.mjs`)

- `e2e/shots/mobile-shots.test.js` and `e2e/shots/store-shots.test.js` do not wrap their tests in `guarded()`: a
  failed screenshot run leaves no `FAIL-*.png`.
- `app.browser.refresh()` instead of `app.reload()` (the old document can still answer `ready`, and keys sent
  then are lost): `e2e/shots/mobile-shots.test.js:37`, `e2e/shots/readme-shots.test.js:44`, `:218`, `:227`,
  `:263`, `:371`.

## Passed on the first run

Stylesheet braces and merge markers, literal colors, weights, layers, focus ring and pill radii as tokens,
uppercase labels, accent frames on selected, current and focused states, glows and gradient frames, left bars
(borders, pseudo elements, inset shadows, bar columns), colored rings, faded text (one decoration, the calendar's
`faint-mark`, allowed as in Arcalo), inline styles in components, neutral focus and selection tokens, endless
animations, `:focus` rings outside `:focus-visible`, contrast of the dark and light themes, icon-only buttons
without a name, button roles, hard-coded UI text (the sample mailbox of the plain-browser preview is listed),
key notation, emojis, mail HTML outside the frame and `allow-scripts`, the release notes' shape, the commit
rules, and the e2e suite's `guarded`, skips, app path, kills and real servers.
