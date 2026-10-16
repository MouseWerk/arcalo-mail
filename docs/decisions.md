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
