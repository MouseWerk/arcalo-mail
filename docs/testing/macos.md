# macOS: manual checklist

What the Linux e2e runs cannot prove: the bundle, Gatekeeper, the Keychain, Touch ID, the default mail app, the
Dock and the menu bar, notifications. Go through it on a Mac with Apple Silicon (and Intel for releases) with the
bundled app of the CI run (not `cargo run`). Note the build and macOS version in the result.

## Install and start

- [ ] The ad-hoc signed app opens after „Öffnen“ in the context menu (or System Settings → Privacy & Security →
      „Trotzdem öffnen“); later starts open without asking.
- [ ] The title bar, the traffic lights and the sidebar line up; full screen and a second Space work.
- [ ] The window at 900, 1280 and 1920 px, on a Retina and a non-Retina display: no blurry text, no clipped labels.
- [ ] Light, dark and contrast themes, and „Kontrast erhöhen“ of macOS: text readable, focus visible.

## Accounts and secrets

- [ ] Add an IMAP account, quit with ⌘Q and start again: no password asked for; the entry is in the Keychain
      under the app's name.
- [ ] Microsoft 365 sign-in in the default browser returns to the app.
- [ ] App lock with Touch ID: locks and unlocks; with the lid closed, the password.

## Desktop integration

- [ ] Settings → Standard-App für E-Mail: a `mailto:` link in Safari opens Arcalo Mail's compose.
- [ ] Notifications with sender and subject; clicking one opens the message; the Dock badge counts unread mail.
- [ ] Closing the window keeps the app in the Dock; clicking the Dock icon brings it back; ⌘Q quits.
- [ ] Shortcuts with ⌘ as listed in Settings → Tastenkürzel; ⌘, opens the settings; ⌘K the palette.
- [ ] Typing with dead keys (⌥U then U gives ü) and with the Japanese input method in compose, search and a
      dialog: Enter picks the candidate and does not send or close, Esc cancels the composition only.
- [ ] Attachments: Quick Look with the space bar, save with the system dialog; a downloaded file is quarantined
      (Gatekeeper asks before opening an app from a mail).
- [ ] VoiceOver: every button is announced with its name, menus as menus; the list reads row by row.

## Updates

- [ ] With a newer release published: the update notice, install, restart; accounts, settings and mail are kept.
