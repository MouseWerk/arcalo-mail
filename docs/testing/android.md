# Android: manual checklist

What the emulator e2e suite (`e2e/android/`, branch `e2e2`) cannot prove: real notifications, the system
share sheet, the Keystore, background checks under Doze, the widget, other launchers. Go through it on a real
phone (a mid-range one, Android 12 or newer) and a tablet with the APK of the CI run, for every release and for
changes in the area. Note the build, device and Android version in the result.

Prepare: two accounts (one IMAP account of the test server reachable over the network, or a real test account,
and one Microsoft 365 account), notifications allowed, the widget on the home screen.

## Start and layout

- [ ] First start: the opening animation is short; with „Animationen entfernen“ in Android's settings it is not
      shown. The setup opens; nothing is cut off in portrait and landscape.
- [ ] Gesture navigation and 3-button navigation: the bottom bar and the compose button stay above the system
      bar; no content under the status bar or the camera cutout.
- [ ] Font size at its largest (Settings → Display → Font size) and display size large: no clipped labels in
      the list, the reading view, compose and settings.
- [ ] Dark theme of the system and the app's own light, dark and contrast themes: no unreadable text.
- [ ] Tablet in landscape: list and reading pane side by side; rotate while a message is open: it stays open.

## Accounts and secrets

- [ ] Add an IMAP account, then force-stop the app and start it again: no password is asked for (Keystore).
- [ ] Microsoft 365 sign-in in the system browser returns to the app; a cancelled sign-in says so.
- [ ] Remove an account: its mail, folders and its entry in the account switcher are gone; adding it again asks
      for the password.
- [ ] Gmail address: the setup shows the steps for an app password and opens Google's page.

## Mail

- [ ] Swipe left and right on a row: the actions of the settings, with undo in the toast.
- [ ] Long press on a row selects; more rows by tapping; bulk archive and delete with undo.
- [ ] Open an HTML mail with remote images: blocked until „Bilder laden“; links open in the browser.
- [ ] Compose with the keyboard open: the toolbar stays above the keyboard; an attachment from the document
      picker and from the camera; send, then undo within the seconds of the setting.
- [ ] Share a photo from the gallery to Arcalo Mail: a new message with the attachment opens.
- [ ] A `mailto:` link in the browser opens compose with the address and subject.
- [ ] Back gesture: closes the open message, then the folder, then leaves the app; never loses a draft.

## Notifications and background

- [ ] Lock the phone, send a mail to the account: one notification, the sender and subject; the badge counts it.
- [ ] Notification actions „Archivieren“ and „Als gelesen markieren“ from the lock screen work and the
      notification goes.
- [ ] After a night in Doze: new mail arrives at the next background check, no duplicates.
- [ ] Turn notifications off for one account: none from it, the other still notifies.

## Widget and updates

- [ ] The widget shows the unread count and the newest messages; tapping one opens it.
- [ ] APK build: with a newer release published, the update notice appears; Google Play build: no notice.

## Accessibility

- [ ] TalkBack: every button is announced with its name, menus as menus; the list can be read row by row.
