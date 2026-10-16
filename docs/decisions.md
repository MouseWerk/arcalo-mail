# Owner decisions

Decided by the owner on 2026-10-16 for the first release.

- **Version:** the first public release is 1.0.0.
- **iOS:** MouseWerk joins the paid Apple Developer Program; Arcalo Mail is distributed through TestFlight and
  the App Store. The owner creates the account and adds the signing secrets to the repository's GitHub
  secrets; until then the iOS job only compiles. This is an exception to Arcalo's "no paid certificates"
  rule, for the App Store only.
- **Android:** Google Play (one-time developer fee) for updates, plus the APK attached to each GitHub
  release. The release keystore stays outside the repository; its password and key go to GitHub secrets.
- **Sign-ins:** Microsoft 365 and Outlook.com sign-in (OAuth) ships in 1.0.0; MouseWerk registers the free
  Entra app (multi-tenant and personal accounts, public client) and verifies the publisher. Google sign-in
  comes later, after Google's verification; until then Gmail works over IMAP with an app password, and the
  account setup says how to create one.
- **Accounts in 1.0.0:** IMAP/SMTP, Microsoft 365/Outlook.com, JMAP.

## Modern features for 1.0.0 (2026-10-17)

All four groups are part of 1.0.0:

- **Inbox that sorts itself:** split inbox (People, Newsletters, Notifications, Receipts), the Screener
  for first-time senders, bundles, one-click unsubscribe (List-Unsubscribe-Post), removal of tracking
  pixels and link trackers. The Screener is off by default and offered in the setup; contacts and
  replies always pass.
- **Time tools:** snooze, send later, follow-up reminders, pin, read later, a done/inbox-zero flow, focus
  mode, unified inbox.
- **AI assistant:** thread summaries, reply drafts, writing help, questions to the mailbox in plain
  words, translation, priority detection, tasks and dates from mails. Local first: a local model
  (Ollama, LM Studio or a built-in small model) by default; a cloud provider only when the user adds
  one, switchable per account; otherwise no mail content leaves the device.
- **Power tools:** templates and snippets, rules, calendar invites (accept/decline), contacts, an
  attachment hub, PGP and S/MIME, sending a mail to Arcalo as a note or task, quick actions in
  notifications, mobile widgets.

## Calendar (2026-10-17)

Arcalo Mail gets a calendar with the same design as Arcalo's Kalender (day, work week, week, month and
list; the same grid, event cards, detail panel, quiet states and tokens, taken over from Arcalo's
`ui/src/views/Calendar*` and adapted). Sources in 1.0.0: Microsoft 365 / Outlook.com (Graph, needs the
delegated permission Calendars.ReadWrite in the Entra app), CalDAV (iCloud, Fastmail, mailbox.org,
Posteo, Nextcloud and others), JMAP Calendars where offered, ICS subscriptions, and invitations in mails
(accept, tentative, decline with the reply to the organizer). Create, move and delete own appointments;
meetings with attendees send updates only after confirmation, as in Arcalo 1.21. Google Calendar comes
with the Google sign-in.
