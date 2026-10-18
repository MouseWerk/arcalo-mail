# Windows: manual checklist

What the Linux e2e runs cannot prove: the NSIS installer, the credential manager, Windows Hello, the default mail
app, notifications, the updater on a real Windows. Go through it on Windows 11 (and Windows 10 for releases) with
the setup of the CI run, on a machine without the Visual C++ Redistributable. Note the build and Windows version
in the result. The checklist of each release adds its own items (`docs/releases/v<version>.md`, „Checklist for
Windows (manual)“).

## Install and start

- [ ] The setup runs without admin rights, puts Arcalo Mail in the Start menu and starts it.
- [ ] SmartScreen shows the unsigned setup („Weitere Informationen“ → „Trotzdem ausführen“); nothing else warns.
- [ ] The window at 900, 1280 and 1920 px and maximized; 100 %, 125 % and 150 % display scaling: no blurry text,
      no clipped labels. Snap layouts on the maximize button work.
- [ ] Light, dark and contrast themes, and Windows' own contrast themes: text readable, focus visible.

## Accounts and secrets

- [ ] Add an IMAP account, quit and start again: no password asked for; the entry is in the Credential Manager
      under the app's name, with no password in `%APPDATA%\de.mousewerk.arcalo.mail`.
- [ ] Microsoft 365 sign-in in the default browser returns to the app.
- [ ] App lock with Windows Hello (face, fingerprint or PIN): locks and unlocks.

## Desktop integration

- [ ] Settings → Standard-App für E-Mail: after the switch, a `mailto:` link in Edge opens Arcalo Mail's compose.
- [ ] Notifications appear in the action center with sender and subject; clicking one opens the message.
- [ ] Attachments: open with the default app, save with the system dialog; a file from the internet keeps its
      „Mark of the Web“ (the properties show the security note).
- [ ] Drag a file from Explorer into compose: it is attached.
- [ ] Keyboard only: every view and dialog reachable, the focus ring visible, Esc closes dialogs.

## Updates and removal

- [ ] With a newer release published: the update notice, install, restart; accounts, settings and mail are kept.
- [ ] Portable mode (`portable` marker next to the program): data stays next to the program.
- [ ] Uninstall from Settings → Apps: the program is gone; the data folder stays unless removal was chosen.
