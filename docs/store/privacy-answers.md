# Data safety (Google Play) and App Privacy (App Store)

The answers for the stores' privacy forms, with the reasons, so they can be given the same way at every update.
They follow from the owner decisions (docs/decisions.md, CLAUDE.md): no own server, no analytics, no crash upload, no
ads; mail moves only between the device and the servers the user set up. The public policy text is
[privacy-policy.md](privacy-policy.md).

The stores count data as "collected" when it leaves the device so that the developer (or a third party acting for
the developer) can access it. Mail sent to the user's own mail provider at the user's direction, sign-ins at
Microsoft, lookups the user starts and a model provider the user adds are the user's own services, not ours.

## Google Play → Policy → App content → Data safety

| Question | Answer | Why |
|---|---|---|
| Does your app collect or share any of the required user data types? | **No** | Nothing reaches MouseWerk or a party of ours. |
| Is all of the user data collected by your app encrypted in transit? | (not asked with "No"; if asked: **Yes**) | TLS or STARTTLS to every server; plain text only to the device itself. |
| Do you provide a way for users to request that their data is deleted? | (not asked with "No"; if asked: **Yes**) | Removing an account deletes its data from the device; uninstalling deletes everything; nothing is kept elsewhere. |
| Independent security review | No | |
| Data deletion URL (if asked) | https://arcalo.mousewerk.de/mail/privacy | Its section „Löschen“ / "Deleting your data". |

If the review insists that data leaving the device counts (it has happened to mail apps), declare exactly this and
nothing more:

- *Personal info → Email address*: collected, **not shared**, not processed ephemerally, **required**, purpose
  **App functionality**.
- *Messages → Emails*: collected, **not shared**, not processed ephemerally, **required**, purpose **App
  functionality**.
- Encrypted in transit: yes. Deletion: yes (as above).

Other Play answers: **Ads** no · **Target audience** 18 and over · **Content rating** (IARC) category *Communication*,
users can exchange messages: yes, no other content · **News app** no · **Government app** no · **Financial
features** none · **Health** none · **App access**: a demo IMAP mailbox for the review. **Permissions** without a
declaration form: INTERNET, ACCESS_NETWORK_STATE, POST_NOTIFICATIONS, RECEIVE_BOOT_COMPLETED. The app does not ask
for REQUEST_INSTALL_PACKAGES: the update notice of the GitHub APK opens the download in the browser, and a copy from
Google Play never checks for updates itself (Play's policy on self-updates; `src-tauri/src/mobile/apknotice.rs`).

## App Store Connect → App Privacy

- **Data collection:** **"No, we do not collect data from this app."** The label then reads *Data Not Collected*.
- **Tracking:** none (no IDFA, no third-party SDKs). `src-tauri/ios/PrivacyInfo.xcprivacy` says the same
  (`NSPrivacyTracking` false, no collected data types) and lists the required-reason APIs: file timestamps, system
  boot time, user defaults, disk space.
- **Privacy policy URL:** https://arcalo.mousewerk.de/mail/privacy
- **Export compliance:** `ITSAppUsesNonExemptEncryption` is false in `src-tauri/Info.ios.plist` (only the standard
  TLS of the system and rustls; OpenPGP and S/MIME for the user's own messages are exempt as standard cryptography in
  a mass-market app). App Store Connect then asks nothing per build.
- **Sign in with Apple:** not required: the app has no account of its own; the Microsoft sign-in is the mail
  provider's.

## Microsoft Store (when it comes)

Privacy policy URL as above; "This product accesses, collects or transmits personal information": **yes** (Partner
Center asks this for any app that handles e-mail), with the policy explaining that nothing goes to the developer.
