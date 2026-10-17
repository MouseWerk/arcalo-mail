# Privacy policy (text for arcalo.mousewerk.de/mail/privacy)

The page the stores, the Microsoft Entra app (docs/microsoft.md) and Google's consent screen link to. The website is
the separate repository; this is the text to publish there, German first (the site's language), then English.
Fill in the bracketed parts before publishing: the controller's postal address and contact e-mail (German law asks
for a reachable address), and the date. Keep it in step with the README's „Privacy“ section when the app changes.

---

## Datenschutzerklärung für Arcalo Mail

Stand: [Datum]

Arcalo Mail ist eine E-Mail-App für Windows, macOS, Linux, Android und iOS. Sie arbeitet lokal: Deine E-Mails,
Kontakte und Termine werden auf deinem Gerät gespeichert und durchsucht. Wir, MouseWerk, betreiben dafür keinen
Server, und die App sendet uns keine Daten.

### Verantwortlich

MouseWerk, Maurice Kleindienst, [Postanschrift], [E-Mail-Adresse für Datenschutzanfragen]

### Was wir erheben

Nichts. Arcalo Mail enthält keine Analyse, keine Werbung, keine Absturzberichte und kein Tracking. Wir erhalten
weder deine E-Mails noch deine Adressen, Passwörter, Kontakte, Termine oder Angaben zur Nutzung der App.

### Was auf deinem Gerät bleibt

- Deine E-Mails, Ordner, Kontakte, Termine, Einstellungen und der Suchindex liegen in einer Datenbank im Datenordner
  der App auf deinem Gerät.
- Passwörter, Anmelde-Token, API-Schlüssel und private Schlüssel für OpenPGP und S/MIME liegen im Schlüsselbund
  des Systems (Windows-Anmeldeinformationsverwaltung, macOS- und iOS-Schlüsselbund, Secret Service unter Linux; unter
  Android mit einem Schlüssel des Android Keystore verschlüsselt). Auf einem Linux-Desktop ohne Schlüsselbund liegen
  sie in einer Datei, die nur dein Benutzer lesen kann; die App sagt das in den Einstellungen.
- Die Einordnung des Posteingangs, die Prüfung neuer Absender, Erinnerungen und die Benachrichtigungen entstehen
  auf dem Gerät.

### Mit wem die App spricht

Die App verbindet sich nur mit den Diensten, die du einrichtest oder ausdrücklich anstößt. Diese Anbieter
verarbeiten die Daten nach ihren eigenen Datenschutzbestimmungen.

- **Deine Mailserver** (IMAP, SMTP, JMAP, POP3) mit deinen Zugangsdaten, um E-Mails abzurufen und zu senden.
- **Microsoft**, wenn du ein Microsoft-365- oder Outlook.com-Konto mit „Mit Microsoft anmelden“ einrichtest: die
  Anmeldung von Microsoft im Browser, IMAP und SMTP von Microsoft und Microsoft Graph für Serverregeln, Kontakte und
  Kalender. Dein Passwort gibst du nur bei Microsoft ein.
- **Beim Einrichten eines Kontos**, um die Server zu finden: die Konfigurationsdatei deines Anbieters, die
  Anbieterdatenbank von Mozilla Thunderbird (autoconfig.thunderbird.net) und DNS-Abfragen zur Domain deiner Adresse.
  Dabei wird die Domain deiner Adresse übertragen, nicht dein Passwort.
- **Adressbücher und Kalender**, die du hinzufügst (CardDAV, CalDAV, ICS-Abonnements, JMAP).
- **Abbestellen**: Nur wenn du auf „Abbestellen“ tippst, ruft die App die Abmeldeadresse des Absenders auf oder
  sendet die E-Mail, die der Verteiler dafür nennt.
- **Absenderbilder**: Für das Logo eines Absenders fragt die App einmal je Domain den BIMI-Eintrag im DNS ab und lädt
  das dort genannte Logo (abschaltbar unter Einstellungen → Kontakte). Gravatar ist aus; wenn du es einschaltest,
  erhält Gravatar einen Hash der Absenderadresse. Bilder aus einer E-Mail werden nie dafür verwendet.
- **Schlüssel von Empfängern** (Web Key Directory): nur wenn du „Nachschlagen“ wählst, bei der Domain des
  Empfängers.
- **Der Assistent** ist aus, bis du einen Anbieter einrichtest: ein Modell auf deinem Computer (Ollama, LM Studio)
  oder ein Cloud-Anbieter, den du selbst hinzufügst (OpenAI, Anthropic, Mistral, Azure OpenAI). Für jedes Konto legst
  du fest, welcher Anbieter dessen E-Mails sehen darf. An den Anbieter gehen nur die Texte, die eine Aktion braucht
  (etwa die Nachrichten einer Unterhaltung für eine Zusammenfassung), und nur, wenn du sie anstößt oder die
  automatische Einordnung „Wichtig“ eingeschaltet ist; die App zeigt bei jeder Aktion, wohin der Text geht.
- **Updates**: Die Desktop-App und die Android-App aus dem GitHub-Release fragen bei GitHub (github.com) nach einer
  neuen Version und laden sie von dort. Dabei sieht GitHub wie bei jedem Abruf einer Webseite deine IP-Adresse.
  Installationen aus Google Play und dem App Store fragen nicht; sie werden vom Store aktualisiert.

Verbindungen zu Servern im Internet sind verschlüsselt (TLS). Externe Bilder in E-Mails werden erst geladen, wenn
du es erlaubst; Tracking-Pixel werden nie geladen.

### Berechtigungen auf dem Handy

Internet und Netzwerkstatus (E-Mails abrufen), Benachrichtigungen (neue E-Mails, nur nach deiner Zustimmung) und
unter Android der Neustart des Abrufs im Hintergrund nach dem Einschalten des Geräts. Keine Standort-, Kontakt-,
Kamera- oder Mikrofonberechtigung.

### Löschen

Entfernst du ein Konto in den Einstellungen, löscht die App seine E-Mails, Daten und Zugangsdaten vom Gerät. Mit
der Deinstallation der App wird alles gelöscht (unter Windows bietet die Deinstallation an, auch den Datenordner zu
löschen). Auf deinen Mailservern bleibt alles, wie es dort ist. Da wir keine Daten von dir haben, gibt es bei uns
nichts zu löschen.

### Kinder

Arcalo Mail richtet sich nicht an Kinder unter 16 Jahren.

### Deine Rechte

Du hast nach der DSGVO das Recht auf Auskunft, Berichtigung, Löschung, Einschränkung, Datenübertragbarkeit und
Widerspruch sowie das Recht, dich bei einer Datenschutzaufsichtsbehörde zu beschweren. Da wir keine
personenbezogenen Daten über die App verarbeiten, betreffen solche Anfragen in der Regel die oben genannten Anbieter,
die du selbst gewählt hast. Fragen beantworten wir unter [E-Mail-Adresse].

### Änderungen

Ändert sich, mit wem die App spricht, passen wir diese Erklärung an und nennen die Änderung in den
Versionshinweisen.

---

## Privacy policy for Arcalo Mail

Last updated: [date]

Arcalo Mail is an e-mail app for Windows, macOS, Linux, Android and iOS. It is local-first: your e-mail, contacts
and appointments are stored and searched on your device. We, MouseWerk, run no server for it, and the app sends us no
data.

### Controller

MouseWerk, Maurice Kleindienst, [postal address], [e-mail address for privacy requests]

### What we collect

Nothing. Arcalo Mail has no analytics, no ads, no crash reports and no tracking. We receive neither your e-mail nor
your addresses, passwords, contacts, appointments or anything about how you use the app.

### What stays on your device

- Your e-mail, folders, contacts, appointments, settings and the search index are kept in a database in the app's
  data folder on your device.
- Passwords, sign-in tokens, API keys and private OpenPGP and S/MIME keys are kept in the system's credential store
  (Windows Credential Manager, the macOS and iOS Keychain, the Secret Service on Linux; on Android encrypted with a
  key of the Android Keystore). On a Linux desktop without a keyring they are kept in a file only your user can read;
  the app says so in its settings.
- Sorting the inbox, screening new senders, reminders and notifications happen on the device.

### Whom the app talks to

The app connects only to the services you set up or explicitly ask for. These providers process data under their own
privacy policies.

- **Your mail servers** (IMAP, SMTP, JMAP, POP3), with your credentials, to fetch and send e-mail.
- **Microsoft**, when you add a Microsoft 365 or Outlook.com account with "Sign in with Microsoft": Microsoft's
  sign-in in the browser, Microsoft's IMAP and SMTP, and Microsoft Graph for server rules, contacts and calendars. You
  enter your password only at Microsoft.
- **While you add an account**, to find its servers: your provider's configuration file, Mozilla Thunderbird's
  provider database (autoconfig.thunderbird.net) and DNS lookups for the domain of your address. Only the domain of
  your address is sent, never your password.
- **Address books and calendars** you add (CardDAV, CalDAV, ICS subscriptions, JMAP).
- **Unsubscribing**: only when you tap "Unsubscribe", the app calls the sender's unsubscribe address or sends the
  e-mail the list asks for.
- **Sender pictures**: for a sender's logo the app looks up the domain's BIMI record in the DNS once and loads the logo
  it names (switch it off under Settings → Contacts). Gravatar is off; if you switch it on, Gravatar receives a hash
  of the sender's address. Pictures inside an e-mail are never used for this.
- **Recipients' keys** (Web Key Directory): only when you choose "Look up", at the recipient's domain.
- **The assistant** is off until you set up a provider: a model on your computer (Ollama, LM Studio) or a cloud
  provider you add yourself (OpenAI, Anthropic, Mistral, Azure OpenAI). For each account you decide which provider
  may see its e-mail. Only the text an action needs goes to the provider (for example the messages of a conversation
  for a summary), and only when you ask for it or switch on "Mark important mail"; the app shows with each action
  where the text goes.
- **Updates**: the desktop app and the Android app from the GitHub release ask GitHub (github.com) for a new version
  and download it from there. GitHub sees your IP address, as with any web request. Installs from Google Play and the
  App Store do not ask; the store updates them.

Connections to servers on the internet are encrypted (TLS). Remote images in e-mail load only when you allow it;
tracking pixels never load.

### Permissions on phones

Internet and network state (fetching e-mail), notifications (new e-mail, only after you agree), and on Android
restarting the background check after the device starts. No location, contacts, camera or microphone permission.

### Deleting your data

When you remove an account in the settings, the app deletes its e-mail, data and credentials from the device.
Uninstalling the app deletes everything (on Windows the uninstaller offers to delete the data folder too). Your mail
servers keep everything as it is there. As we hold no data about you, there is nothing to delete on our side.

### Children

Arcalo Mail is not directed at children under 16.

### Your rights

Under the GDPR you have the right of access, rectification, erasure, restriction, data portability and objection,
and the right to lodge a complaint with a data protection authority. As we process no personal data through the app,
such requests usually concern the providers listed above that you chose yourself. Questions: [e-mail address].

### Changes

If the services the app talks to change, we update this policy and name the change in the release notes.
