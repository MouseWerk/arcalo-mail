# Microsoft 365 and Outlook.com sign-in

Arcalo Mail signs in to Microsoft accounts with OAuth 2.0 as a public client: the authorization code flow
with PKCE in the system browser, the code delivered to a loopback address of the computer
(`http://127.0.0.1:<random port>`, RFC 8252), no client secret in the app. IMAP
(`outlook.office365.com:993`) and SMTP (`smtp.office365.com:587`) then authenticate with XOAUTH2 and an
access token. The refresh token is kept in the system's credential store
(`account-<id>-oauth-refresh-token`), access tokens only in memory; they are refreshed five minutes before
they expire and once more when a server refuses one. Code: `crates/mail-core/src/oauth.rs`, the setup in
`ui/src/components/Setup.tsx`.

Everything works without an app registration except the sign-in itself: a build without a client id says
"This build of Arcalo Mail has no Microsoft sign-in" and offers IMAP with an app password instead.

## What the app asks for

| | |
|---|---|
| Authority | `https://login.microsoftonline.com/common/oauth2/v2.0` (work, school and personal accounts) |
| Scopes | `offline_access`, `https://outlook.office.com/IMAP.AccessAsUser.All`, `https://outlook.office.com/SMTP.Send`, `openid`, `email`, `profile` |
| Redirect on the desktop | `http://127.0.0.1:<port>` (any port; no path) |
| Redirect on Android and iOS | `de.mousewerk.arcalo.mail://oauth` (see below) |
| Client id | compiled in from the build environment variable `ARCALO_MAIL_MS_CLIENT_ID` |

## Owner steps: register the app in Microsoft Entra

Free; any Microsoft Entra tenant works (a Microsoft 365 business tenant of MouseWerk, or the free tenant that
comes with an Azure account). Sign in at <https://entra.microsoft.com> with an account that may register
applications.

1. **Identity → Applications → App registrations → New registration.**
   - Name: `Arcalo Mail` (users see it on the consent screen).
   - Supported account types: **Accounts in any organizational directory (Any Microsoft Entra ID tenant –
     Multitenant) and personal Microsoft accounts (e.g. Skype, Xbox)**.
   - Redirect URI: platform **Public client/native (mobile & desktop)**, value `http://localhost`.
   - Register. Note the **Application (client) ID** on the overview page.
2. **Authentication.**
   - Under **Mobile and desktop applications** keep `http://localhost` and add
     `de.mousewerk.arcalo.mail://oauth` (the phone apps of milestone 3).
   - The portal does not accept `http://127.0.0.1` in this box. Add it in **Manifest** instead: in
     `replyUrlsWithType`, add an entry `{ "url": "http://127.0.0.1", "type": "InstalledClient" }` next to
     the existing ones and save. The port is ignored for loopback addresses, so every port the app picks
     matches.
   - **Advanced settings → Allow public client flows: Yes.**
   - No client secret and no certificate: the app is a public client.
3. **API permissions → Add a permission → Microsoft Graph → Delegated permissions**: `IMAP.AccessAsUser.All`,
   `SMTP.Send`, `offline_access`, `openid`, `email`, `profile` (the app requests them with the full scope
   strings above, `https://outlook.office.com/…`, as Microsoft's IMAP/SMTP OAuth guide asks; listing them
   here shows them on the consent screen and lets administrators consent ahead). No application
   permissions (`…AccessAsApp`): those are for services, not for a mail app. Do not grant admin consent in
   MouseWerk's tenant on behalf of others; every organization consents for itself (see below).
4. **Branding & properties.** Publisher domain `mousewerk.de` (verify it with the DNS TXT record Entra
   shows), home page `https://arcalo.mousewerk.de`, and the links to the privacy statement and the terms
   (needed before many organizations allow the app).
5. **Publisher verification** (Branding & properties → Publisher verification). Needs a Microsoft AI Cloud
   Partner Program (formerly MPN) ID – free – whose account is verified and tied to the same verified
   domain. Without it users see "unverified" on the consent screen and tenants with the default user
   consent settings block the app (users then get the admin consent error below).

## Owner steps: put the client id into the builds

The client id of a public client is not a secret; it is in every installed copy anyway. It is set as a
GitHub Actions **variable**, not as a secret:

1. GitHub → MouseWerk/arcalo-mail → **Settings → Secrets and variables → Actions → Variables → New
   repository variable**.
2. Name `ARCALO_MAIL_MS_CLIENT_ID`, value the Application (client) ID from step 1, e.g.
   `0a1b2c3d-…`.

`.github/workflows/ci.yml` passes `${{ vars.ARCALO_MAIL_MS_CLIENT_ID }}` to every job as the environment
variable of the same name; `oauth.rs` reads it at compile time (`option_env!`). The release workflow, once
it exists, needs the same line. For a local build: `ARCALO_MAIL_MS_CLIENT_ID=… cargo tauri build`.

## What users and their IT may run into

- **"Your organization has to approve Arcalo Mail"** (AADSTS65001, AADSTS90094): the tenant does not let
  users consent to third-party apps. An administrator signs in once and approves the app for the
  organization (Entra admin center → Enterprise applications, or the admin consent prompt). Publisher
  verification lowers how often this happens.
- **"Sending is switched off for this account"** (`535 5.7.139`): Exchange Online turns SMTP AUTH off for
  many mailboxes. IT switches on *Authenticated SMTP* for the user (Microsoft 365 admin center → Users →
  Active users → Mail → Manage email apps) or for the tenant. Reading works meanwhile; the setup offers to
  add the account without sending.
- **Signing in again**: after a password change, a revoked grant or 90 days without use the refresh token
  ends; the account shows "Sign-in needed" and *Sign in* opens the browser again.

## Phones

Android and iOS cannot listen on a loopback port while the browser is in front. They use the redirect
`de.mousewerk.arcalo.mail://oauth` (registered above) through the platform's browser session: a Custom Tab on
Android (`AuthTab.kt`; the activity's intent filter for the scheme hands the redirect to Rust), an
`ASWebAuthenticationSession` on iOS (`mobile/ios.rs`; it shares Safari's sign-ins). `oauth::sign_in_with_session`
does the rest as on the desktop: PKCE, the state check, the code exchange. Closing the browser without signing in
counts as cancelled. The core's test `signs_in_through_the_mobile_redirect` runs the flow against the fake identity
provider.

## Tests

`crates/mail-core/src/oauth/fake.rs` is an identity provider in memory (authorize, token, refresh, PKCE
check, rotating refresh tokens, admin consent and cancel). The core tests run the whole flow against it on
loopback; the test server (`crates/mail-testserver`) serves it on its control port and accepts its access
tokens for XOAUTH2 on IMAP and SMTP (`POST /smtp-auth?disabled=1` imitates `535 5.7.139`). The e2e test
`e2e/tests/providers.test.js` signs in through the app with `ARCALO_MAIL_MS_AUTHORITY` pointing at the test
server; the app accepts such an authority, and a client id from the environment, only on 127.0.0.1.
