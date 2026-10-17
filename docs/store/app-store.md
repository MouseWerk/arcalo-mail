# App Store: listing

For App Store Connect → the app → the version page and App Information (English (U.S.) as the primary language,
German as a localization). Limits: name 30 characters, subtitle 30, promotional text 170, keywords 100 (comma
separated, no spaces needed), description 4,000. The account, certificates and TestFlight are in
[../release-mobile.md](../release-mobile.md); the App Privacy answers in [privacy-answers.md](privacy-answers.md).

- **Name:** Arcalo Mail
- **Bundle ID:** de.mousewerk.arcalo.mail · **SKU:** arcalo-mail
- **Primary category:** Productivity · **Secondary category:** Business
- **Age rating:** 4+ (no objectionable content of its own, not a web browser)
- **Price:** free, no in-app purchases
- **Support URL:** https://arcalo.mousewerk.de/mail/
- **Marketing URL:** https://arcalo.mousewerk.de/mail/
- **Privacy policy URL:** https://arcalo.mousewerk.de/mail/privacy ([privacy-policy.md](privacy-policy.md))
- **Copyright:** 2026 MouseWerk, Maurice Kleindienst
- **Screenshots:** iPhone 6.9" `screenshots/iphone-6.9/` (1320 × 2868), iPhone 6.5" `screenshots/iphone-6.5/`
  (1284 × 2778), iPad 13" `screenshots/ipad-13/` (2064 × 2752), from `cd e2e && npm run shots:store`. The sample
  mailboxes are English; the same pictures serve the German localization.
- **Review notes:** a demo IMAP mailbox (address, password, servers if not found by the address); new mail is checked
  while the app is open and by background app refresh otherwise (no push server, no account with us); the
  assistant is off until a model is set up and is not needed for the review.

Arcalo Mail cannot be chosen as the default mail app on iOS: that needs Apple's mail client entitlement, which is
asked for separately (Apple's „Default Apps“ program) once the app is in the store. Not requested for 1.0.0.

## English

**Subtitle** (20 characters): Calm, private e-mail

**Promotional text** (163 characters)

> Your mail on your device: a sorted inbox, a calendar with meeting invitations, OpenPGP and S/MIME. No telemetry,
> no ads, remote images blocked until you load them.

**Keywords** (98 characters)

> mail,email,imap,smtp,jmap,outlook,gmail,calendar,caldav,pgp,smime,privacy,inbox,newsletter,offline

**Description**

> Arcalo Mail is a calm, local-first e-mail app from the makers of Arcalo. It keeps a copy of your mailboxes on your
> iPhone and iPad and works on that copy: the inbox opens at once, search answers while you type, and reading,
> archiving and writing go on without a connection. Changes reach your mail server in the background as soon as it
> can be reached.
>
> No telemetry, no ads, no server of ours in between. The app talks to your mail provider and to nothing else you
> did not set up. Remote images are blocked until you load them, and tracking pixels and link trackers are removed
> from every message.
>
> ACCOUNTS
> • Any IMAP/SMTP provider – iCloud, GMX, WEB.DE, T-Online, mailbox.org, Posteo, Yahoo and many more, found by your
> address
> • Microsoft 365 and Outlook.com with the Microsoft sign-in, no password in the app
> • Gmail and Google Workspace with an app password, with a guide in the setup
> • JMAP (Fastmail and others) and POP3
>
> AN INBOX THAT SORTS ITSELF
> • Tabs for people, newsletters, notifications and receipts, sorted on your device
> • New senders wait until you let them in, if you like
> • Unsubscribe in one tap where the sender supports it
> • Snooze, send later, reminders when nobody answered, pin and read later
>
> CALENDAR AND MEETINGS
> • Day, week, month and list, from CalDAV (iCloud, Fastmail, mailbox.org, Nextcloud), Microsoft 365 and ICS
> • Invitations as cards with Accept, Tentative, Decline and Join for Teams, Zoom, Webex and Meet
>
> PRIVATE BY DESIGN
> • Mail, search and sorting stay on the device; passwords live in the iOS Keychain
> • OpenPGP and S/MIME: sign, encrypt, decrypt and verify
> • An optional assistant (summaries, reply drafts, writing help) with a model you choose; off until you set one up
>
> MADE FOR IPHONE AND IPAD
> • Swipe to archive or mark read, pull to refresh, the actions within reach of your thumb
> • On iPad the list and the message side by side
> • New mail in the background when iOS allows, with Archive, Mark read and Reply on the notification
> • Light, dark and high-contrast themes, Dynamic Type, English and German
>
> Arcalo Mail is open source (MIT license): https://github.com/MouseWerk/arcalo-mail

**What's New (1.0.0):** The first release of Arcalo Mail.

## Deutsch

**Untertitel** (22 Zeichen): Ruhige, private E-Mail

**Werbetext** (169 Zeichen)

> Deine Mails auf deinem Gerät: ein sortierter Posteingang, ein Kalender mit Einladungen, OpenPGP und S/MIME. Keine
> Telemetrie, externe Bilder blockiert, bis du sie lädst.

**Schlüsselwörter** (94 Zeichen)

> mail,email,e-mail,imap,outlook,gmail,kalender,caldav,pgp,smime,datenschutz,posteingang,offline

**Beschreibung**

> Arcalo Mail ist eine ruhige E-Mail-App, die lokal zuerst arbeitet – von den Machern von Arcalo. Sie hält eine
> Kopie deiner Postfächer auf iPhone und iPad und arbeitet mit dieser Kopie: Der Posteingang öffnet sofort, die
> Suche antwortet beim Tippen, und Lesen, Archivieren und Schreiben gehen auch ohne Verbindung. Änderungen
> erreichen deinen Mailserver im Hintergrund, sobald er erreichbar ist.
>
> Keine Telemetrie, keine Werbung, kein Server von uns dazwischen. Die App spricht mit deinem Mailanbieter und mit
> nichts, was du nicht selbst eingerichtet hast. Externe Bilder bleiben blockiert, bis du sie lädst, und
> Tracking-Pixel und Link-Tracker werden aus jeder Nachricht entfernt.
>
> KONTEN
> • Jeder IMAP/SMTP-Anbieter – iCloud, GMX, WEB.DE, T-Online, mailbox.org, Posteo, Yahoo und viele mehr, anhand
> deiner Adresse gefunden
> • Microsoft 365 und Outlook.com mit der Microsoft-Anmeldung, ohne Passwort in der App
> • Gmail und Google Workspace mit einem App-Passwort, mit Anleitung bei der Einrichtung
> • JMAP (Fastmail und andere) und POP3
>
> EIN POSTEINGANG, DER SICH SELBST SORTIERT
> • Tabs für Personen, Newsletter, Benachrichtigungen und Belege, sortiert auf deinem Gerät
> • Neue Absender warten, bis du sie zulässt, wenn du willst
> • Abbestellen mit einem Tippen, wo der Absender es unterstützt
> • Zurückstellen, später senden, Erinnerungen ohne Antwort, Anheften und Später lesen
>
> KALENDER UND BESPRECHUNGEN
> • Tag, Woche, Monat und Liste, aus CalDAV (iCloud, Fastmail, mailbox.org, Nextcloud), Microsoft 365 und ICS
> • Einladungen als Karten mit Zusagen, Mit Vorbehalt, Absagen und Beitreten für Teams, Zoom, Webex und Meet
>
> PRIVAT VON ANFANG AN
> • Mails, Suche und Sortierung bleiben auf dem Gerät; Passwörter liegen im iOS-Schlüsselbund
> • OpenPGP und S/MIME: signieren, verschlüsseln, entschlüsseln und prüfen
> • Ein Assistent, wenn du willst (Zusammenfassungen, Antwortentwürfe, Schreibhilfe), mit einem Modell deiner Wahl;
> aus, bis du eines einrichtest
>
> GEMACHT FÜR IPHONE UND IPAD
> • Wischen zum Archivieren oder Als-gelesen-Markieren, Ziehen zum Aktualisieren, die Aktionen in Daumenreichweite
> • Auf dem iPad Liste und Nachricht nebeneinander
> • Neue Mails im Hintergrund, wenn iOS es zulässt, mit Archivieren, Gelesen und Antworten in der Benachrichtigung
> • Helles, dunkles und kontrastreiches Design, dynamische Schriftgröße, Deutsch und Englisch
>
> Arcalo Mail ist Open Source (MIT-Lizenz): https://github.com/MouseWerk/arcalo-mail

**Neuerungen (1.0.0):** Die erste Version von Arcalo Mail.
