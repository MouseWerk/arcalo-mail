// Send later with the app closed: a message whose time came while the app was not running goes
// out at its next start (one app at a time: a second instance would only bring the first one up).

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";

let app;
let server;
let dataDir;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const env = { ARCALO_MAIL_TICK_SECS: "1" };

before(async () => {
  server = await startServer();
  dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-e2e-later-"));
});

after(async () => {
  await app?.close();
  server?.stop();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

it("sends at the next start what was due while the app was closed", async () => {
  app = await launch({ env, dataDir });
  await addAccount(app, server);
  const accounts = await app.invoke("list_accounts");
  const at = Math.floor(Date.now() / 1000) + 6;
  const draft = { account_id: accounts[0].id, to: ["lea@example.org"], cc: [], bcc: [], subject: "Beim nächsten Start", text: "x", html: null, attachments: [], in_reply_to: null, references: [], message_id: null, reply_to_id: null, forwarded: [] };
  await app.invoke("send_later", { draft, at, followupSecs: null });
  await app.close();
  app = null;
  await sleep(9000);
  assert.ok(!(await server.sent()).some((m) => m.subject === "Beim nächsten Start"), "nothing while closed");
  app = await launch({ env, dataDir });
  await app.waitUntil(async () => (await server.sent()).some((m) => m.subject === "Beim nächsten Start"), 40000, "sent at the start");
});

