# 1.1.0 plan: quality in every area

The first public release of Arcalo Mail is 1.1.0 (`docs/decisions.md`). Owner, 2026-10-17: „Und immer wieder
testen, so dass es fully polished ist wie Arcalo. Baue genau die gleichen Vorschriften und Tests auf.“ The rules
and guards are Arcalo's (`docs/quality/README.md`); this plan lists the areas, how each is checked, the bar it
has to reach, and its status. Findings go to `findings.md` in this folder (severity, repro, root cause, fix,
test).

Rounds running in parallel when this plan was written: `and` (phone UI and account switcher), `gm` (sync speed),
`e2e2` (Android emulator e2e), Arcalo's `ios`. Areas they own are checked again after their merge (`gm` and `and` are merged on 2026-10-18).

## Rules for every area

- The checks of `CLAUDE.md` green; every guard of the README green (the ones red until `and` included, after its
  merge).
- Each fix gets a test where practical (Rust unit test, vitest, e2e); root causes, no retries, never skip or
  weaken a test.
- Visual pass in every area: screenshots across the matrix below, looked at one by one; every picture shows the
  whole window or one whole dialog.
- Migrations take the next number (latest on `main`: 0012), settings changes a step
  (`SETTINGS_VERSION` 12 on `main`).

## Exit criterion

**3 consecutive fully clean rounds.** A round: every area below checked against its bar; the full desktop e2e
suite run 5 times in a row without a single failure (`e2e soak` workflow, input `rounds` 5); the Android emulator
suite green; CI green on Linux, Windows and macOS; the manual checklists of `docs/testing/` for every area changed
since the last round. A round is clean when it found nothing to fix. Any fix starts the count again.

| Round | Date | Soak (5x) | Android e2e | Findings | Clean |
|-------|------|-----------|-------------|----------|-------|
| 0 | 2026-10-17/18 | not yet run (workflow added; it can be started once it is on `main`) | waits for `e2e2` | guards: `findings.md` (all fixed on `qa`, after the merges of `gm` and `and`); CI run 39068876509: one e2e failure in `mobile-tools`, passed on rerun, root cause `browser.refresh()` (fixed) | no |

## Device and theme matrix

Every area with a UI is checked across this matrix (screenshots and the layout audit for the desktop sizes; the
phone and tablet sizes on emulators and simulators):

| Dimension | Values |
|-----------|--------|
| Android | emulator phones: small (360x640 dp), common (411x891 dp, Pixel 8), large (430x932 dp); tablet (1280x800 dp, both orientations); gesture and 3-button navigation |
| iOS | simulators: iPhone SE (3rd gen), iPhone 16, iPhone 16 Pro Max; iPad (10th gen) and iPad Pro 13", both orientations, Split View and Slide Over |
| Desktop | Windows 11, macOS 14+ (Apple Silicon), Linux (GNOME on Wayland and X11); window widths 900, 1280, 1440 and 1920 px; split panes (reading pane right and below, the list at its narrowest) |
| Themes | light, dark, contrast |
| Languages | de (du), en |
| Font size | 100 % and 130 % (system font scaling on phones, the app's zoom on desktops) |
| Accounts | 1 account, 3 accounts (IMAP, Microsoft 365, JMAP; the unified inbox and the account switcher) |
| Data | a large Gmail-like account: 100,000 messages in the inbox, 300 labels as folders, threads of 200 messages, 5 GB of attachments (the test server's Gmail mode of `e2e2`, `crates/mail-testserver/src/gmail.rs`) |

## Areas

| # | Area | Method | Bar | Status |
|---|------|--------|-----|--------|
| A1 | Sync (IMAP, POP3, JMAP, Microsoft Graph; IDLE and push; offline queue) | Read `engine.rs`, `sync.rs`, `outbox.rs` end to end after `gm`; conformance against Dovecot and GreenMail (CI); fault injection in the test server (drops, slow answers, UIDVALIDITY change, expunge during fetch, a folder renamed elsewhere); e2e offline, restart and large mailbox; the soak | No message lost, duplicated or shown twice in any fault; local actions survive a restart and apply once; first sync of the large account shows the newest mail in under 10 s and stays responsive; no busy CPU while idle | open; after `gm` |
| A2 | Accounts (setup, presets, Gmail app password, Microsoft sign-in, POP3, account switcher, removal) | Every provider preset against the test server; the Microsoft sign-in with the fake OAuth server; removal leaves no secret and no data; switcher with 1 and 3 accounts | A user with any listed provider gets from address to inbox without reading docs; every error says what to do; nothing of a removed account stays (credential store, database, files) | open; switcher after `and` |
| A3 | Phone UI (Android, iOS) | The `e2e2` emulator suite; simulator runs for iOS; `docs/testing/android.md`, `ios.md`; screenshots in the phone part of the matrix | Arcalo's phone app level: thumb reach, safe areas and insets, back gesture, no clipped text at 130 %, 60 fps scrolling of a 10,000 message list on a mid-range phone | open; after `and` and `e2e2` |
| A4 | Desktop UI (list, reading pane, compose, palette, settings, split panes) | Visual pass across the desktop matrix with the layout audit (clipped text, overflow, overlaps: `e2e/lib/geometry.js` of `e2e2`); keyboard-only walk through every view | Obsidian/Linear-level polish in light, dark and contrast at 900–1920 px; every action reachable by keyboard; no layout jumps | open |
| A5 | Calendar (views, sources, invitations, writes, conflicts) | Arcalo's Kalender checks (q121 plan) applied: create, move, delete, meeting updates only after confirmation; CalDAV, Graph, JMAP, ICS fixtures; time zones and DST | Same look and behavior as Arcalo's Kalender; no write without confirmation for meetings; time zone and DST edge cases correct | open |
| A6 | Crypto and security (PGP, S/MIME, app lock, encryption at rest, phishing warning, sanitizer, remote content, credential store) | Review `sanitize.rs`, `mailframe.ts`, `crypto/`, `cipher/`, `applock.rs`, `phishing/`; malicious mail corpus (scripts, forms, CSS exfiltration, tracking pixels, lookalike domains); guards of the README | No script runs, no remote load without consent, no secret in database, settings or logs; lock and encryption as Arcalo's | open |
| A7 | Rules and AI (local and server rules, Sieve, Graph rules, assistant, local and cloud providers) | Rule preview vs result on fixtures; Sieve and Graph round trips; AI with the fake providers; offline and refused providers | A rule catches exactly what its preview showed; no mail content leaves the device without a cloud provider the user added for that account | open |
| A8 | Notifications (desktop, Android, iOS; quick actions, badges, quiet hours) | Unit tests of the planner; manual checklists per platform | One notification per new message, none for read or old ones, actions work from the lock screen, badges match the unread count | open |
| A9 | Updates and stores (desktop updater, APK feed, Google Play, TestFlight and App Store, Microsoft Store and winget) | Update feed tests, the updater in release mode (CI), a real update from the previous build on each desktop OS (checklists), store builds | An update never loses data or settings, never runs an unsigned package; store builds pass their store's checks (WACK for MSIX) | open; MSIX job planned |
| A10 | i18n (de with du, en) | Translation check, hard-coded text check; English e2e run with a German leftovers check (port Arcalo's `germanLeftovers`) | No German in the English UI and no English in the German one; glossary terms only; texts fit at 130 % | open |
| A11 | Accessibility | Guards (names, menus, IME, focus-visible, contrast); screen reader walk (NVDA, VoiceOver, TalkBack) per platform checklist; reduced motion | Every control named and reachable; focus always visible on keyboard use; 4.5:1 text in all themes | open; guards in place |
| A12 | Performance | Large account of the matrix; start time, list scroll, search, opening a thread, memory over a day; budgets in e2e (`budget`) | Start to inbox under 1.5 s on desktop (3 s on a mid-range phone); search under 300 ms on 100,000 messages; no leak over 24 h | open |

## TODO

- **Android emulator e2e in CI.** When `e2e2` is merged, hook its emulator job (`.github/workflows/ci.yml`,
  `android.yml` on that branch) into the round: it has to be green in every round, and the soak gets an input to
  run the Android suite N times too. Nothing of `e2e2` is changed from this branch.
- **Merge order.** `qa` (with `gm` and `and` merged in, every guard green) into `main`, then round 1 starts with
  the soak.
- **Planned guards** (README table, status planned): themes tests with Arcalo's themes, the settings fixture of
  1.1.0, the desktop-only command list, the MSIX job, the English leftovers check.
