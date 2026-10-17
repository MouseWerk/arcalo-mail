# Releasing Arcalo Mail

A release is built and published by `.github/workflows/release.yml`, as Arcalo's: Windows, macOS and Linux
installers, the update feed the desktop app reads, the Android APK with its own feed, and the iOS build for
TestFlight. `auto-release.yml` starts it by itself once CI is green on `main` and a release notes file for a new
version is there. This page is what the owner does once, how a release goes, and what users get.

Never put a private key, a password or a certificate into the repository, a chat, an issue or a commit. They go
into the GitHub secrets and an offline backup only.

## What a release contains

| File | For |
|---|---|
| `ArcaloMail_<version>_x64-setup.exe` | Windows 10/11: per-user setup, no administrator rights, WebView2 bootstrapper inside, C runtime linked statically; updates itself |
| `ArcaloMail_<version>_x64-portable.zip` | Windows: the program with the marker file `arcalo-mail-portable`; data in `data\` next to it; updated by hand |
| `ArcaloMail_<version>_aarch64.dmg`, `…_x64.dmg` | macOS 11+ on Apple Silicon and Intel, ad-hoc signed (no paid certificate, not notarized); updates itself once it runs from Applications |
| `…_aarch64.app.tar.gz`, `…_x64.app.tar.gz` (+ `.sig`) | the macOS update files the app downloads |
| `ArcaloMail_<version>_amd64.AppImage` (+ `.sig`) | Linux; updates itself (replaces the file in place) |
| `ArcaloMail_<version>_amd64.deb`, `…_x86_64.rpm` | Linux packages; updated through the release page or the package manager |
| `ArcaloMail_<version>_x64-setup.exe.sig` | the Windows update signature |
| `latest.json` | the desktop update feed: version, notes, the file and signature per platform |
| `ArcaloMail_<version>_android.apk` (+ `.sig`), `android-latest.json` | Android, installed by hand from the release; the app tells about newer APKs |

The AAB for Google Play is not attached: it is the artifact `arcalo-mail-android` of the release run (Actions →
Release → the run → Artifacts), uploaded to Play Console by hand ([release-mobile.md](release-mobile.md)). The iOS
build goes to TestFlight from the run itself.

## Once: the update key

Every desktop update and the GitHub APK are signed with one key pair (minisign, Tauri's updater format). The public
key is compiled into the app; the app installs only files signed with the private key. Whoever has the private
key can ship code to every installed copy, and without it no installed copy can ever be updated again (people
would have to install by hand). Make it once, on a computer you trust, and keep it forever.

1. Install the Tauri command line if it is not there: `cargo install tauri-cli --version "^2" --locked`.
2. Generate the pair (it asks for a password; choose a strong one):

   ```
   cargo tauri signer generate -w ~/.tauri/arcalo-mail.key
   ```

   This writes `~/.tauri/arcalo-mail.key` (the private key, encrypted with the password) and
   `~/.tauri/arcalo-mail.key.pub` (the public key). Both files are one line of base64.
3. Back up `arcalo-mail.key` and its password in two offline places (a password manager and an encrypted
   backup). Do not commit it, do not paste it anywhere else than the secret fields below.
4. GitHub → MouseWerk/arcalo-mail → **Settings → Secrets and variables → Actions → New repository secret**:

   | Secret | Value |
   |---|---|
   | `TAURI_SIGNING_PRIVATE_KEY` | the content of `~/.tauri/arcalo-mail.key` (`cat ~/.tauri/arcalo-mail.key`, the whole line) |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password from step 2 |

5. Put the **public** key into `src-tauri/tauri.conf.json`: replace `REPLACE_WITH_THE_UPDATE_PUBLIC_KEY` in
   `plugins.updater.pubkey` with the content of `~/.tauri/arcalo-mail.key.pub` (`cat ~/.tauri/arcalo-mail.key.pub`),
   commit and push. The public key is not secret; it is in every installed copy.

The release workflow refuses to start (job „Update key“, before anything is tagged) while the placeholder is
there, while the key is not a minisign public key, or while `TAURI_SIGNING_PRIVATE_KEY` is missing. Normal CI does
not need the key: its builds have no updater and never contact the feed.

The release compiles the public key into the app as `ARCALO_MAIL_UPDATER_PUBKEY` and signs the bundles
(`bundle.createUpdaterArtifacts`); local builds and CI builds stay without it.

## Once: variables and the phones

- **`ARCALO_MAIL_MS_CLIENT_ID`** (repository variable): the Microsoft sign-in, [microsoft.md](microsoft.md). Without
  it the release has no Microsoft sign-in.
- **`AUTO_RELEASE`** (repository variable, optional): `false` switches the automatic release off (below).
- **Android and iOS**: the keystore and the Apple secrets of [release-mobile.md](release-mobile.md). Without them the
  release has no APK (a debug-signed APK is not attached) and no TestFlight build; the desktop release goes on.

## Releasing

1. Write `docs/releases/v<version>.md`. The first release uses `## Arcalo Mail 1.0.0` with its highlights and a
   `## Checklist (manual)` with a part per platform; later ones use `## Improved`, `## Fixed` and
   `## Checklist for Windows (manual)`. User-facing lines only. The file becomes the tag's message, the GitHub
   release's text and the notes in `latest.json`; the app shows them in „Neu in Arcalo Mail“ after the update,
   without the checklist sections and without a title line that starts with „Arcalo Mail“.
2. Commit it with the last changes of the release, or after them, and push to `main`.
3. CI runs; when it is green, „Auto release“ starts `release.yml` for that commit. Watch it under Actions → Auto
   release (the release is its job „Release“).
4. Afterwards: the website (changelog, version, screenshots), Play Console (the AAB), App Store Connect (the
   TestFlight build to review), and the manual checklist of the notes.

Do not push a notes file of a version before it is meant to be released: the first green CI after it releases.

Other ways to start it: Actions → Release → **Run workflow** with the version (`1.0.0`; the tag is created on the
selected branch), or pushing a tag `v1.0.0` yourself.

### The automatic release

`auto-release.yml` releases after CI passed for a push to `main` when all of these hold, else it ends with a
notice:

- the CI run succeeded and ran for a push to `main` of this repository, and its commit is still `main`'s head;
- the newest `docs/releases/v<major>.<minor>.<patch>.md` (by version) is newer than every release tag and has no
  tag yet (older untagged notes and pre-release names do not count; `scripts/release-pick.mjs`, tested by
  `scripts/release-pick.test.mjs` in CI);
- the variable `AUTO_RELEASE` is not `false`.

Before the first release there is no tag and no `docs/releases` folder: nothing happens. Its runs form one queue,
and `release.yml` refuses an existing tag when the automatic release calls it, so nothing is released twice.

### A failed release

The tag exists then, so the automatic release does not try again. Fix the cause and run Actions → Release → Run
workflow with the same version: it builds the existing tag again and replaces the release's files.

### What the release checks

- The version is `MAJOR.MINOR.PATCH`; it is written into `Cargo.toml`, `src-tauri/tauri.conf.json` and
  `ui/package.json` (`scripts/set-version.mjs`), so the app, the bundles and Android's version code agree.
- Windows: the program imports none of the Visual C++ Redistributable's DLLs (`scripts/check-windows-runtime.ps1`).
- `latest.json`: every platform's file is there and its signature verifies against the public key of
  `tauri.conf.json` and names the version (`scripts/verify-update-feed.mjs`), before anything is published.
- `android-latest.json` is written only for an APK signed with the release key, with its package, version code
  and certificate read from the APK (`scripts/apk-signer.mjs`).

## How users are updated

| Install | Update |
|---|---|
| Windows setup | Downloaded in the background; installed when the app quits or with „Jetzt neu starten“ (the setup runs passively and starts the new version). Settings → Über: „Automatisch“ or „Nur benachrichtigen“. |
| Windows portable | The app tells about the new version and opens the release page; unpack the new ZIP over the folder. |
| macOS from Applications | As on Windows; the app bundle is replaced (with the administrator's password where the folder needs it). |
| macOS started from the disk image or the Downloads folder | macOS runs it from a read-only copy: the app says so and asks to move it to Applications. |
| Linux AppImage | As on Windows; the file is replaced in place. A folder without write permission: the app says so. |
| Linux .deb or .rpm | The app tells about the new version and opens the release page; or the package manager. |
| Android, APK from the release | The app looks at `android-latest.json` at the start and once a day and offers „Herunterladen“; Android's installer asks before it replaces the app. |
| Android from Google Play, iOS | The store updates the app; the app never asks a feed. |

Before installing, an update waits up to a minute for messages that are being sent; an open message is saved as a
draft first. After the update the app shows „Neu in Arcalo Mail“ once.

## First start of the unsigned installers

There is no paid code signing certificate (owner decision). Users see:

- **Windows SmartScreen**: „Der Computer wurde durch Windows geschützt“ → „Weitere Informationen“ → „Trotzdem
  ausführen“. The warning fades as the setup gains reputation. The Microsoft Store build (below) avoids it.
- **macOS Gatekeeper** (ad-hoc signed, not notarized): open the disk image, drag Arcalo Mail to Applications, then
  open it once with a right click → „Öffnen“ (macOS 14 and earlier), or after the first attempt in System
  Settings → Privacy & Security → „Trotzdem öffnen“ (macOS 15).

The README and the website say this next to the downloads.

## Microsoft Store (MSIX), later

The free signed path for Windows, as Arcalo's `docs/release/microsoft-store.md`: the Store signs the package, so
SmartScreen does not warn and the Store updates the app. Not built yet. The steps when it comes:

1. A Partner Center developer account (MouseWerk), the app name „Arcalo Mail“ reserved; note the product
   identity (Package/Identity/Name, Publisher, PublisherDisplayName) and the Store ID.
2. A Store build without the app's own updater and portable mode (Arcalo's `store` feature): the Store updates the
   package.
3. The MSIX: `AppxManifest.xml` with the identity, `runFullTrust`, the `windows.protocol` extension for `mailto`
   (so Windows lists Arcalo Mail under Default apps without the setup's registry keys), the icons; packed with
   `makeappx` as Arcalo's `packaging/msix/pack.ps1`, unsigned (the Store signs it).
4. A `store` job in `release.yml` as Arcalo's, building the MSIX when the repository variables `STORE_IDENTITY_NAME`,
   `STORE_PUBLISHER` and `STORE_PUBLISHER_DISPLAY_NAME` exist, and submitting it with the Microsoft Store Developer
   CLI when the Partner Center secrets exist.
5. The listing: the texts and pictures of [store/](store/) fit (Store screenshots 1366 × 768 or larger of the
   desktop app, `docs/screenshots`).

winget (Arcalo's `docs/release/winget.md`) follows the same way: the first version by hand, then a job with the
secret `WINGET_TOKEN`.

## Checking a release by hand

```
# The update feed and its files in one folder (from the release page):
node scripts/verify-update-feed.mjs <folder> "$(node -p 'require("./src-tauri/tauri.conf.json").plugins.updater.pubkey')" 1.0.0
# The APK's signing certificate:
node scripts/apk-signer.mjs ArcaloMail_1.0.0_android.apk
```
