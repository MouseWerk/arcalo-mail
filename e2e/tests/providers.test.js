// The ways in besides IMAP with a password: Microsoft 365 / Outlook.com through the browser
// sign-in (the test server's identity provider, XOAUTH2 on IMAP and SMTP), what a build without
// a Microsoft client id says, the Gmail app-password guide, and JMAP found at the provider's
// well-known address. The "browser" is this test: it reads the address the app opened and
// follows it.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { message, startServer } from "../lib/server.js";
import { clickText, waitRow } from "../lib/mail.js";

let app;
const servers = [];
const it = guarded(test, () => app);

after(async () => {
  await app?.close();
  for (const s of servers) s.stop();
});

async function fresh(env) {
  await app?.close();
  followed = 0;
  app = await launch({ env });
  await app.waitFor(".setup-welcome");
}

let followed = 0;

/** Plays the browser: follows the sign-in address the app opened last. */
async function browserSignIn(authority) {
  let url = null;
  await app.waitUntil(
    async () => {
      const list = app.opened().filter((u) => u.startsWith(authority));
      url = list.length > followed ? list[list.length - 1] : null;
      return url !== null;
    },
    15000,
    "the sign-in was not opened in the browser",
  );
  followed += 1;
  const r = await fetch(url);
  await r.text();
}

it("signs in with Microsoft in the browser and reads and sends with XOAUTH2", async () => {
  const ms = await startServer({ user: "maria@outlook.com" });
  servers.push(ms);
  await ms.deliver(message({ to: "Maria <maria@outlook.com>", subject: "Willkommen bei Outlook", body: "Hallo Maria" }));
  await fresh(ms.microsoftEnv());
  const authority = ms.microsoftEnv().ARCALO_MAIL_MS_AUTHORITY;

  await app.fill(".setup-fields input[type=email]", "maria@outlook.com");
  await app.waitText(".setup-note", /Microsoft-Konto/);
  assert.equal((await app.$$(".setup-fields input[type=password]")).length, 0, "no password for Microsoft");

  // The organization has not approved the app yet: said plainly.
  await ms.oauthFail("admin_consent");
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".setup-microsoft");
  await browserSignIn(authority);
  await app.waitText(".problem-title", /Organisation muss Arcalo Mail freigeben/, 20000);
  await app.shot("microsoft-admin-consent");

  // Approved: the sign-in goes through and the servers are checked with the token.
  await clickText(app, ".setup-microsoft .btn-primary", "Erneut versuchen");
  await browserSignIn(authority);
  await app.waitFor(".setup-done", 20000);
  await app.click(".setup-done .btn-primary");
  await waitRow(app, "Willkommen bei Outlook");
  const [account] = await app.invoke("list_accounts");
  assert.equal(account.provider, "microsoft");
  assert.deepEqual(account.transport.auth, { kind: "o_auth2", provider: "microsoft" });

  // Sending with an access token.
  await app.invoke("send_message", {
    draft: {
      account_id: account.id,
      to: ["jonas@example.org"],
      cc: [],
      bcc: [],
      subject: "Per Microsoft gesendet",
      text: "Hallo Jonas",
      html: null,
      attachments: [],
      in_reply_to: null,
      references: [],
      message_id: null,
      reply_to_id: null,
      forwarded: [],
    },
  });
  await app.waitUntil(async () => (await ms.sent()).some((m) => m.subject === "Per Microsoft gesendet"), 30000, "sent over SMTP with XOAUTH2");
  const status = (await app.invoke("list_accounts"))[0].status;
  assert.equal(status?.status?.state, "ready");
});

it("says when the build has no Microsoft sign-in, and guides Gmail users to an app password", async () => {
  await fresh({});
  const info = await app.invoke("app_info");
  await app.fill(".setup-fields input[type=email]", "maria@outlook.com");
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".setup-microsoft");
  if (info.microsoft_sign_in) {
    // A build with a client id (ARCALO_MAIL_MS_CLIENT_ID set while building) opens the browser.
    await app.waitText(".setup-microsoft .btn-primary", /Im Browser anmelden|Erneut versuchen/);
  } else {
    await app.waitText(".setup-microsoft .notice", /keine Microsoft-Anmeldung/);
    await app.shot("microsoft-not-in-build");
  }
  await clickText(app, ".setup-microsoft .btn", info.microsoft_sign_in ? "Zurück" : "Mit App-Passwort einrichten");

  await app.fill(".setup-fields input[type=email]", "maria@gmail.com");
  await app.waitFor(".guide");
  assert.match(await app.text(".guide"), /Bestätigung in zwei Schritten/);
  assert.match(await app.text(".setup-fields"), /App-Passwort/);
  await clickText(app, ".guide .btn", "App-Passwörter öffnen");
  await app.waitUntil(async () => app.opened().some((u) => u.includes("myaccount.google.com/apppasswords")), 10000, "app passwords opened");
  await app.shot("gmail-guide");
});

it("finds JMAP at the well-known address and syncs with push", async () => {
  const jm = await startServer();
  servers.push(jm);
  await jm.deliver(message({ subject: "Über JMAP angekommen", body: "Hallo" }));
  await fresh(jm.discoveryEnv());
  await app.fill(".setup-fields input[type=email]", jm.user);
  await app.fill(".setup-fields input[type=password]", jm.password);
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".setup-done", 30000);
  await app.click(".setup-done .btn-primary");
  await waitRow(app, "Über JMAP angekommen");
  const [account] = await app.invoke("list_accounts");
  assert.equal(account.provider, "jmap");
  assert.match(account.transport.session_url, /\/\.well-known\/jmap$/);
  // New mail arrives through the EventSource push.
  await jm.deliver(message({ subject: "Gleich gepusht", body: "x" }));
  await waitRow(app, "Gleich gepusht", true, 20000);
});
