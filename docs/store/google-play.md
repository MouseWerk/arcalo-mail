# Google Play: store listing

For Play Console → Grow → Store presence → Main store listing (English (United States) as the default language,
German (Germany) as a translation). Limits: app name 30 characters, short description 80, full description 4,000.
The other steps (app access, content rating, data safety, testing tracks) are in
[../release-mobile.md](../release-mobile.md); the data safety answers in [privacy-answers.md](privacy-answers.md).

- **App name:** Arcalo Mail
- **Category:** Communication · **Tags:** Email, Calendar, Productivity
- **Contact:** the MouseWerk support address · **Website:** https://arcalo.mousewerk.de/mail/
- **Privacy policy:** https://arcalo.mousewerk.de/mail/privacy ([privacy-policy.md](privacy-policy.md))
- **App icon:** 512 × 512, from `src-tauri/icons/icon.png` (Arcalo's icon is still the placeholder, docs/plan.md)
- **Feature graphic:** 1024 × 500, not made yet (the icon on the app's background color with the name)
- **Phone screenshots:** `screenshots/play-phone/*.png`, 1080 × 1920 (9:16, inside Play's 2:1 limit). The sample
  mailboxes are English; the same pictures serve the German listing.
- **Tablet screenshots (optional):** `screenshots/ipad-13/` as they are (2064 × 2752; Play takes up to 3840 px on the
  long side)
- The pictures come from `cd e2e && npm run shots:store` (`e2e/shots/store-shots.test.js`): the app's touch layout at
  the store's size and pixel ratio with the sample mailboxes of the README on a fixed clock.

## English

**Short description** (68 characters)

> Calm, private e-mail with a calendar. Your mail stays on your phone.

**Full description**

> Arcalo Mail is a calm, local-first e-mail app from the makers of Arcalo. It keeps a copy of your mailboxes on your
> phone and works on that copy: the inbox opens at once, search answers while you type, and reading, archiving and
> writing go on without a connection. Changes reach your mail server in the background as soon as it can be
> reached.
>
> No telemetry, no ads, no server of ours in between. The app talks to your mail provider and to nothing else you
> did not set up. Remote images are blocked until you load them, and tracking pixels and link trackers are removed
> from every message.
>
> ACCOUNTS
> • Any IMAP/SMTP provider (GMX, WEB.DE, T-Online, mailbox.org, Posteo, iCloud, Yahoo and many more, found by your
> address)
> • Microsoft 365 and Outlook.com with the Microsoft sign-in, no password in the app
> • Gmail and Google Workspace with an app password, with a guide in the setup
> • JMAP (Fastmail and others) and POP3
>
> AN INBOX THAT SORTS ITSELF
> • Tabs for people, newsletters, notifications and receipts, sorted on your phone
> • New senders wait until you let them in (optional)
> • Unsubscribe in one tap where the sender supports it
> • Snooze, send later, reminders when nobody answered, pin and read later
>
> CALENDAR AND MEETINGS
> • Day, week, month and list, from CalDAV (iCloud, Fastmail, mailbox.org, Nextcloud), Microsoft 365 and ICS
> • Invitations as cards with Accept, Tentative, Decline and Join for Teams, Zoom, Webex and Meet
>
> PRIVATE BY DESIGN
> • Mail, search and sorting stay on the device; passwords are sealed with a key of the Android Keystore
> • OpenPGP and S/MIME: sign, encrypt, decrypt and verify
> • An optional assistant (summaries, reply drafts, writing help) with a model you choose; off until you set one up
>
> MADE FOR PHONES
> • Swipe to archive or mark read, pull to refresh, the actions within reach of your thumb
> • New mail in the background about every 15 minutes, with Archive, Mark read and Reply on the notification
> • A home-screen widget with the unread count and the newest mail from people
> • Light, dark and high-contrast themes, English and German
>
> Arcalo Mail is open source (MIT license): https://github.com/MouseWerk/arcalo-mail

**Feature list** (for the website and press)

- Local-first: mail stored and searched on the device, works offline
- No telemetry, no ads, no tracking; remote images blocked, trackers removed
- IMAP/SMTP, Microsoft 365/Outlook.com (sign-in with Microsoft), Gmail (app password), JMAP, POP3
- Sorted inbox with tabs, new-sender screening, one-tap unsubscribe
- Snooze, send later, follow-up reminders, pin, read later
- Calendar (CalDAV, Microsoft 365, ICS) with meeting invitations
- OpenPGP and S/MIME
- Optional assistant with a model of your choice
- Background check, notification actions, home-screen widget
- Light, dark and high-contrast themes; English and German

## Deutsch

**Kurzbeschreibung** (75 Zeichen)

> Ruhige, private E-Mail-App mit Kalender. Deine Mails bleiben auf dem Handy.

**Vollständige Beschreibung**

> Arcalo Mail ist eine ruhige E-Mail-App, die lokal zuerst arbeitet – von den Machern von Arcalo. Sie hält eine
> Kopie deiner Postfächer auf dem Handy und arbeitet mit dieser Kopie: Der Posteingang öffnet sofort, die Suche
> antwortet beim Tippen, und Lesen, Archivieren und Schreiben gehen auch ohne Verbindung. Änderungen erreichen
> deinen Mailserver im Hintergrund, sobald er erreichbar ist.
>
> Keine Telemetrie, keine Werbung, kein Server von uns dazwischen. Die App spricht mit deinem Mailanbieter und mit
> nichts, was du nicht selbst eingerichtet hast. Externe Bilder bleiben blockiert, bis du sie lädst, und
> Tracking-Pixel und Link-Tracker werden aus jeder Nachricht entfernt.
>
> KONTEN
> • Jeder IMAP/SMTP-Anbieter (GMX, WEB.DE, T-Online, mailbox.org, Posteo, iCloud, Yahoo und viele mehr, anhand
> deiner Adresse gefunden)
> • Microsoft 365 und Outlook.com mit der Microsoft-Anmeldung, ohne Passwort in der App
> • Gmail und Google Workspace mit einem App-Passwort, mit Anleitung bei der Einrichtung
> • JMAP (Fastmail und andere) und POP3
>
> EIN POSTEINGANG, DER SICH SELBST SORTIERT
> • Tabs für Personen, Newsletter, Benachrichtigungen und Belege, sortiert auf deinem Handy
> • Neue Absender warten, bis du sie zulässt (wenn du willst)
> • Abbestellen mit einem Tippen, wo der Absender es unterstützt
> • Zurückstellen, später senden, Erinnerungen ohne Antwort, Anheften und Später lesen
>
> KALENDER UND BESPRECHUNGEN
> • Tag, Woche, Monat und Liste, aus CalDAV (iCloud, Fastmail, mailbox.org, Nextcloud), Microsoft 365 und ICS
> • Einladungen als Karten mit Zusagen, Mit Vorbehalt, Absagen und Beitreten für Teams, Zoom, Webex und Meet
>
> PRIVAT VON ANFANG AN
> • Mails, Suche und Sortierung bleiben auf dem Gerät; Passwörter sind mit einem Schlüssel des Android Keystore
> versiegelt
> • OpenPGP und S/MIME: signieren, verschlüsseln, entschlüsseln und prüfen
> • Ein Assistent, wenn du willst (Zusammenfassungen, Antwortentwürfe, Schreibhilfe), mit einem Modell deiner Wahl;
> aus, bis du eines einrichtest
>
> GEMACHT FÜR DAS HANDY
> • Wischen zum Archivieren oder Als-gelesen-Markieren, Ziehen zum Aktualisieren, die Aktionen in Daumenreichweite
> • Neue Mails im Hintergrund etwa alle 15 Minuten, mit Archivieren, Gelesen und Antworten in der Benachrichtigung
> • Ein Widget für den Startbildschirm mit der Zahl der ungelesenen Mails und den neuesten Mails von Personen
> • Helles, dunkles und kontrastreiches Design, Deutsch und Englisch
>
> Arcalo Mail ist Open Source (MIT-Lizenz): https://github.com/MouseWerk/arcalo-mail

**Funktionsliste**

- Lokal zuerst: Mails auf dem Gerät gespeichert und durchsucht, auch offline
- Keine Telemetrie, keine Werbung, kein Tracking; externe Bilder blockiert, Tracker entfernt
- IMAP/SMTP, Microsoft 365/Outlook.com (Anmeldung mit Microsoft), Gmail (App-Passwort), JMAP, POP3
- Sortierter Posteingang mit Tabs, Prüfung neuer Absender, Abbestellen mit einem Tippen
- Zurückstellen, später senden, Erinnerungen, Anheften, Später lesen
- Kalender (CalDAV, Microsoft 365, ICS) mit Besprechungseinladungen
- OpenPGP und S/MIME
- Ein Assistent, wenn du willst, mit einem Modell deiner Wahl
- Abruf im Hintergrund, Aktionen in Benachrichtigungen, Widget
- Helles, dunkles und kontrastreiches Design; Deutsch und Englisch
