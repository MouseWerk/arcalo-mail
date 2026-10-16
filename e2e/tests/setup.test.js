// First start: the welcome screen, setup finds the servers through autoconfig (served by the
// test server), the connection check, a wrong password with a clear error, then the inbox.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { message, startServer } from "../lib/server.js";

let app;
let server;
const it = guarded(test, () => app);

before(async () => {
  server = await startServer();
  await server.deliver(message({ subject: "Willkommen im Team", body: "Hallo Maria,\n\nschön, dass du da bist.\n\nJonas" }));
  app = await launch({ env: { ARCALO_MAIL_AUTOCONFIG: `http://127.0.0.1:${server.control}/autoconfig?emailaddress={email}` } });
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("shows the welcome screen on the first start", async () => {
  await app.waitFor(".setup-welcome");
  assert.match(await app.text(".setup-intro h1"), /Willkommen/);
  await app.shot("setup-welcome");
});

it("explains a wrong password", async () => {
  await app.fill(".setup-fields input[type=email]", server.user);
  await app.fill(".setup-fields input[type=password]", "falsch");
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".problem", 20000);
  const text = await app.text(".problems");
  assert.match(text, /Passwort/);
  assert.match(await app.text(".setup-found"), /127\.0\.0\.1/);
  await app.shot("setup-wrong-password");
});

it("adds the account and opens the inbox", async () => {
  await app.fill(".setup-fields input[type=password]", server.password);
  await app.click(".setup-fields button[type=submit]");
  await app.waitFor(".setup-done", 20000);
  await app.shot("setup-done");
  await app.click(".setup-done .btn-primary");
  await app.waitText(".row-subject", /Willkommen im Team/, 20000);
  await app.shot("inbox-first");
  const accounts = await app.invoke("list_accounts");
  assert.equal(accounts.length, 1);
  assert.equal(accounts[0].email, server.user);
});
