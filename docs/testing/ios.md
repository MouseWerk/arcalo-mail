# iOS and iPadOS: manual checklist

What the simulators cannot prove: the Keychain on a device, push and background refresh, the share sheet,
Face ID, TestFlight updates. Go through it on an iPhone and an iPad with the TestFlight build of the release
candidate. Note the build, device and iOS version in the result.

Prepare: two accounts (IMAP and Microsoft 365), notifications allowed, Background App Refresh on.

## Start and layout

- [ ] First start: the opening animation is short; with „Bewegung reduzieren“ it is not shown. Nothing sits under
      the Dynamic Island, the notch or the home indicator.
- [ ] Larger Text at its largest (Accessibility → Display & Text Size): no clipped labels in the list, the reading
      view, compose and settings.
- [ ] Light and dark appearance of the system, and the app's light, dark and contrast themes.
- [ ] iPad: list and reading pane side by side; Split View and Slide Over at every width; rotate with a message
      open: it stays open. A hardware keyboard: the shortcuts of Settings → Tastenkürzel work with ⌘.

## Accounts and secrets

- [ ] Add an IMAP account, quit the app from the app switcher and start it again: no password is asked for
      (Keychain).
- [ ] Microsoft 365 sign-in in the browser sheet returns to the app; cancelling says so.
- [ ] App lock with Face ID: locks after the set time, unlocks with Face ID and with the PIN.
- [ ] Remove an account: its mail and folders are gone; adding it again asks for the password.

## Mail

- [ ] Swipe actions with undo; long press shows the context menu.
- [ ] HTML mail with remote images blocked until allowed; links open in Safari.
- [ ] Compose: the keyboard never covers the text field or the send button; an attachment from Files and from
      Photos; send and undo.
- [ ] Share a photo from Photos to Arcalo Mail: compose opens with it.
- [ ] Swipe back from the left edge closes the open message; a draft is never lost.

## Notifications and background

- [ ] Locked phone: one notification per new message; the icon badge matches the unread count.
- [ ] Notification actions from the lock screen work and remove the notification.
- [ ] After some hours in the background: new mail arrives with Background App Refresh, no duplicates.

## Distribution

- [ ] TestFlight: installing a newer build keeps accounts, settings and mail.
- [ ] VoiceOver: every button is announced with its name, menus as menus; the list reads row by row.
