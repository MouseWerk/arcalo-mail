# Releasing the Android and iOS apps

Owner decisions (docs/decisions.md): Android through Google Play plus the APK attached to each GitHub release,
iOS through TestFlight and the App Store; the first release is 1.0.0. This page is what the owner does outside the
code, once, and which GitHub secrets the CI jobs read. Never paste a key, a password or a certificate into a chat,
an issue or a commit: they go into the GitHub secrets and an offline backup only.

## What CI does

`.github/workflows/ci.yml` on every push (through `android.yml` and `ios.yml`, which the release calls too):

| Job | Without the secrets | With the secrets |
|---|---|---|
| **Android** | a universal APK (arm64, armv7, x86_64) signed with a throwaway debug key: installs for a test, never as an update of a release | a release APK and an AAB (the format Google Play takes), both signed with the release key |
| **iOS** | the Rust library for the device and the simulator compiles (no Apple account needed) | the Xcode project is made (`cargo tauri ios init`), the app is archived and exported for App Store Connect, the IPA is kept as a build artifact; on a tag `v*` it is uploaded to TestFlight |

A tag `vMAJOR.MINOR.PATCH` is the version: the Android versionName is `1.0.0` and its versionCode `1000000`
(major × 1 000 000 + minor × 1 000 + patch, derived by Tauri), the iOS version `1.0.0` with the workflow's run
number as the build number. The files are the run's artifacts `arcalo-mail-android` (`ArcaloMail_<version>_android.apk`
and `.aab`) and `arcalo-mail-ios`.

The release (`release.yml`, [release.md](release.md)) builds the APK and the AAB of its tag, attaches the
release-signed APK with `android-latest.json` to the GitHub release (the app installed from there tells about newer
versions; a copy from Google Play never checks, and when Android cannot name the installer the notice stays off),
keeps the AAB as the run's artifact `arcalo-mail-android`, and uploads the iOS build to TestFlight. The AAB is a build
of its own without the update notice (no feature `apk-updates`, no update key): it holds no code that looks for
updates, as Play's policy wants. Google Play gets the AAB by hand until a Play service account is set up (see the end
of this page).
The listing texts, the privacy policy text and the data safety answers are in [store/](store/).

## Android: Google Play

### 1. The release key (once, kept forever)

Every update must be signed with the same key. Make it on a computer you trust, with Java installed:

```
keytool -genkeypair -v -keystore arcalo-mail-release.jks -alias arcalo-mail \
  -keyalg RSA -keysize 4096 -validity 10000
```

Choose a strong store password and key password (keytool asks; for a PKCS12 keystore both are the same). Keep
`arcalo-mail-release.jks` and the passwords in two offline places (a password manager and an encrypted backup).
Losing the key means the GitHub APK can never be updated again, and Play needs a key reset through its support.

### 2. Google Play developer account

1. <https://play.google.com/console/signup>. The fee is a one-time 25 USD.
2. **Account type.**
   - **Organization** (recommended for MouseWerk): needs a **D-U-N-S number** of the business (free from Dun &
     Bradstreet, takes a few working days; the same number Apple asks for), a website and a contact. Organization
     accounts may publish to production right away.
   - **Personal**: identity verification of the person. Personal accounts created after 13 November 2023 must run a
     **closed test with at least 12 testers who stay opted in for at least 14 days in a row** before Google allows
     the production release (Play Console → Dashboard → "Apply for production"). Plan for this: invite 12 or more
     people (their Google accounts) to the closed testing track as soon as the first AAB is uploaded.
3. Verify the developer contact email and phone, and the address that is shown on the store page for organizations.

### 3. The app in Play Console

1. **Create app**: name *Arcalo Mail*, default language English (United States) or German, *App*, *Free*; accept the
   declarations.
2. **Play App Signing** (on the first upload of an AAB, Test and release → Setup → App signing). Two choices:
   - *Let Google create the app signing key* (the default): the key of step 1 becomes the **upload key**. Installs
     from Play are then signed with Google's key, the APK on GitHub with ours, so a person cannot switch between
     the two without uninstalling (Android refuses an update with another signature).
   - *Use my own key* (recommended here): "Export and upload a key from Java keystore", run Google's PEPK tool on
     `arcalo-mail-release.jks` as the page shows, upload the encrypted key. Play and the GitHub APK then share the
     signature and update each other. Optionally register a separate upload key afterwards.
3. **Store listing** (Grow → Store presence → Main store listing):
   - Short description and full description: [store/google-play.md](store/google-play.md).
   - App icon 512 × 512 (`docs/brand/png/icon-512.png`), feature graphic 1024 × 500
     (`docs/brand/png/play-feature-graphic-1024x500.png`).
   - Phone screenshots: 2 to 8, each side 320–3840 px and the long side at most twice the short side:
     `docs/store/screenshots/play-phone` (1080 × 1920, made by `cd e2e && npm run shots:store`). Tablet screenshots
     are optional (`docs/store/screenshots/ipad-13`).
   - Category *Communication*; contact email; website <https://arcalo.mousewerk.de>.
4. **Privacy policy** (Policy → App content → Privacy policy): a public web page (not a PDF), for example
   `https://arcalo.mousewerk.de/mail/privacy`. The same page serves the Microsoft Entra app (docs/microsoft.md)
   and App Store Connect. It says: mail and sign-in data stay on the device; the app connects only to the mail
   servers and sign-in services the user sets up; no analytics, no ads, no crash reports; notifications are made
   on the device; how to delete everything (remove the account in the app, or uninstall).
5. **App access**: the app needs a mail account to show anything. Give the reviewers a test mailbox (an IMAP
   account at a provider of your choice, with its address, password and server if not found automatically) under
   "All or some functionality is restricted".
6. **Ads**: no. **Content rating** (IARC questionnaire): category *Communication or social*; users can exchange
   messages (e-mail) – yes; no violence, no gambling, no purchases. **Target audience**: 18 and over (keeps the app
   out of the Families policy). **News app**: no. **Government app**: no. **Financial features**: none. **Health**:
   none.
7. **Data safety** (Policy → App content → Data safety). Answers for this app:
   - *Does your app collect or share any of the required user data types?* — **No.** MouseWerk receives nothing:
     the app has no server, no analytics and no crash upload; mail moves only between the phone and the servers the
     user configured, at the user's direction, and notifications are created on the phone.
   - If the review asks for it to be declared anyway (Google counts data that leaves the device as "collected"):
     *Personal info → Email address* and *Messages → Emails*, collected, **not shared**, *processed ephemerally: no*,
     *required* (the app's purpose), purpose **App functionality** only.
   - *Is all of the user data encrypted in transit?* — **Yes** (TLS or STARTTLS to every server; plain text only to
     the phone itself, which never happens in normal use).
   - *Do you provide a way for users to request that their data be deleted?* — **Yes**: removing the account in
     Settings deletes its mail and credentials from the phone; uninstalling deletes everything; nothing is kept
     elsewhere.
   - Independent security review: no.
8. **Permissions** the app asks for (no declaration form needed): INTERNET, ACCESS_NETWORK_STATE, POST_NOTIFICATIONS
   (asked once an account exists), RECEIVE_BOOT_COMPLETED (WorkManager reschedules the background check after a
   restart). No foreground service, no exact alarms, no SMS or call log.

### 4. GitHub secrets for Android

GitHub → MouseWerk/arcalo-mail → Settings → Secrets and variables → Actions → **New repository secret**:

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | the keystore as one line of base64: `base64 -w0 arcalo-mail-release.jks` (Linux) or `base64 -i arcalo-mail-release.jks` (macOS) – paste the output, no line breaks |
| `ANDROID_KEYSTORE_PASSWORD` | the store password |
| `ANDROID_KEY_ALIAS` | `arcalo-mail` (the alias of step 1) |
| `ANDROID_KEY_PASSWORD` | the key password (for PKCS12 the same as the store password) |

The next CI run signs the APK and builds the AAB. Check: Actions → the run → artifact `arcalo-mail-android`
contains `…_android.aab`; `apksigner verify --print-certs` on the APK shows your certificate.

### 5. The first upload and the testing tracks

1. Push the tag of the release (`v1.0.0`) or take the AAB of the latest main run.
2. Play Console → Test and release → Testing → **Internal testing** → Create release → upload the AAB, release
   notes from `docs/releases/v<version>.md`. Add yourself as an internal tester; install through the opt-in link.
3. Personal account: **Closed testing** with the 12+ testers for 14 days (see step 2), then apply for production.
4. **Production**: promote the release; the first review takes a few days.
5. Later: a Play service account (Google Cloud → service account with the "Release manager" role in Play Console
   → Users and permissions) as the secret `PLAY_SERVICE_ACCOUNT_JSON` lets CI upload the AAB to the internal track
   itself; not set up yet.

## iOS: TestFlight and the App Store

### 1. Apple Developer Program

1. <https://developer.apple.com/programs/enroll/> with an Apple Account that has two-factor authentication.
   99 USD (99 EUR) a year.
2. Enroll as an **organization**: needs the **D-U-N-S number** of MouseWerk as a legal entity (Apple's D-U-N-S
   lookup offers to request one; allow up to two weeks), a website on the business's domain, and the authority to
   sign for it. A sole proprietorship that is not a legal entity cannot enroll as an organization; it enrolls as an
   **individual**, and the App Store then shows the person's name as the seller.
3. Note the **Team ID** (Membership details).

### 2. Identifier, certificate and profile

At <https://developer.apple.com/account> → Certificates, Identifiers & Profiles:

1. **Identifiers → +** → App IDs → App → description *Arcalo Mail*, **Explicit** bundle ID
   `de.mousewerk.arcalo.mail`. No extra capabilities: background fetch and the sign-in session need none (push
   notifications are not used; there is no push server).
2. **Certificates → +** → **Apple Distribution**. The certificate request can come from Keychain Access on a Mac
   (Certificate Assistant → Request a Certificate from a Certificate Authority, saved to disk) or from OpenSSL:
   ```
   openssl req -new -newkey rsa:2048 -nodes -keyout distribution.key -out distribution.csr \
     -subj "/emailAddress=<your Apple Account email>/CN=MouseWerk/C=DE"
   ```
   Upload the `.csr`, download `distribution.cer`, and make the `.p12` with a password of your choice:
   ```
   openssl x509 -inform DER -in distribution.cer -out distribution.pem
   openssl pkcs12 -export -legacy -inkey distribution.key -in distribution.pem -out distribution.p12
   ```
   (On a Mac: export the certificate with its private key from Keychain Access as `.p12`.) Keep the `.p12` and its
   password offline; the certificate is valid for a year and is renewed the same way.
3. **Profiles → +** → Distribution → **App Store Connect** → the App ID → the distribution certificate → name
   *Arcalo Mail App Store* → download the `.mobileprovision`. A renewed certificate needs a new profile.

### 3. The app in App Store Connect

1. <https://appstoreconnect.apple.com> → Apps → **+** → New App: platform iOS, name *Arcalo Mail* (must be free on
   the store), primary language, bundle ID `de.mousewerk.arcalo.mail`, SKU `arcalo-mail`, full access.
2. **App Store Connect API key**: Users and Access → Integrations → App Store Connect API → Team Keys → **Generate
   API Key**, name *CI*, access **App Manager**. Download the `.p8` (only possible once), note the **Key ID** and
   the **Issuer ID** shown above the list.
3. **App Privacy** (the privacy label): **Data Not Collected**. Apple counts data as collected when it leaves the
   device so that the developer or a partner can access it beyond serving the request; Arcalo Mail sends mail only
   to the user's own mail servers, has no server, no analytics and no crash upload. Privacy policy URL: the page of
   the Android section, step 3.4.
4. **Age rating**: no objectionable content of the app's own, no unrestricted web access (it is not a browser) →
   4+.
5. **Export compliance**: answered in the app (`ITSAppUsesNonExemptEncryption = false` in `src-tauri/Info.ios.plist`:
   only the standard TLS of the system and rustls); no question per build.
6. **Privacy manifest**: `src-tauri/ios/PrivacyInfo.xcprivacy` is copied into the project by CI (no tracking, no
   collected data, the reasons for file dates, the system's uptime, the app's preferences and disk space).
7. Screenshots: 6.9-inch iPhone (1320 × 2868), 6.5-inch iPhone (1284 × 2778) and, as the app runs on iPad, 13-inch
   iPad (2064 × 2752): `docs/store/screenshots/` (`cd e2e && npm run shots:store`, the touch layout at the store's
   size and pixel ratio with the sample mailboxes). Texts: [store/app-store.md](store/app-store.md).

### 4. GitHub secrets for iOS

| Secret | Value |
|---|---|
| `APPLE_TEAM_ID` | the Team ID (10 characters) |
| `IOS_CERTIFICATE_BASE64` | `base64 -w0 distribution.p12` (macOS: `base64 -i distribution.p12`) |
| `IOS_CERTIFICATE_PASSWORD` | the password of the `.p12` |
| `IOS_PROVISIONING_PROFILE_BASE64` | `base64 -w0 "Arcalo_Mail_App_Store.mobileprovision"` |
| `APP_STORE_CONNECT_API_KEY_ID` | the Key ID |
| `APP_STORE_CONNECT_ISSUER_ID` | the Issuer ID |
| `APP_STORE_CONNECT_API_KEY_BASE64` | `base64 -w0 AuthKey_<KEYID>.p8` |

The `ios` job then builds the IPA on every push and uploads it to TestFlight for a tag. (fastlane *match* with a
certificates repository is the alternative to the three certificate secrets; it is not set up because one person
signs.) The secret `APPLE_DEVELOPMENT_TEAM` of milestone 1 is no longer read; it can be deleted.

### 5. TestFlight and the review

1. After a tag's upload, App Store Connect processes the build (about 15 minutes; an email follows).
2. **Internal testing**: TestFlight → Internal Testing → a group with your App Store Connect users (up to 100);
   they install through the TestFlight app at once.
3. **External testing** (up to 10 000 people, a public link possible) needs the *Test Information* (what to test,
   feedback email, a demo mail account for the reviewer) and a short beta review.
4. **App Store**: the version page → the build, the description, keywords, support URL, the screenshots, the review
   notes: a demo mailbox (address, password, servers), and that new mail is checked while the app is open and by
   background app refresh otherwise (no push server). Submit for review.

## What only a device or CI can show

- That the Kotlin and Objective-C parts work: the Keystore cipher, Custom Tabs and the redirect, WorkManager,
  `ASWebAuthenticationSession`, `BGAppRefreshTask`, the notifications and the badge. The Rust side of them is
  type-checked for both targets and its logic is tested on the desktop (fake cipher, the sign-in against the fake
  identity provider, `check_once` against the fake server); the e2e tests run the touch layout in the desktop app.
- How often Android's WorkManager runs the check (Doze and the battery saver delay it) and when iOS grants
  background app refresh (the system learns from use; often hours apart, never when the user swiped the app away).
- The notification buttons and the widget (round 3): on Android, Archive and Mark read from a notification with the
  app closed (the store changes, a check follows when a network is there), Reply opening the compose sheet, the
  widget's count and its three rows after a background check, a tap on a row opening that message; on iOS, the
  `new-mail` category's buttons on a notification from the background check (long press), and notifications still
  shown while the app is in front (the app's delegate replaces the notification plugin's).
