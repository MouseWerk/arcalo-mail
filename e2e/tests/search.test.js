// Search: the local index finds subjects, senders and the text of fetched messages at once; a
// message whose body is still on the server is found with „Auf dem Server suchen“.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, subjects, waitRow } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

before(async () => {
  server = await startServer();
  const now = Math.floor(Date.now() / 1000);
  await server.deliver(message({ from: "Lea Sommer <lea@example.org>", subject: "Gartenplan", body: "Die Tomaten kommen an die Südwand." }), { date: now - 400 });
  await server.deliver(message({ from: "Jonas Berger <jonas@example.org>", subject: "Kino", body: "Freitag um acht?" }), { date: now - 300 });
  // Too large to be fetched ahead: its text is only on the server.
  const filler = `${"Protokoll der Sitzung. ".repeat(14000)}\nDas Zauberwort steht ganz am Ende.`;
  await server.deliver(message({ from: "Verwaltung <info@verein.example>", subject: "Langes Protokoll", body: filler }), { date: now - 200 });
  app = await launch();
  await addAccount(app, server);
  await app.reload();
  await waitRow(app, "Langes Protokoll");
  // The bodies of the small messages are in the index.
  await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 2, 20000, "bodies fetched");
});

after(async () => {
  await app?.close();
  server?.stop();
});

it("finds text in fetched messages, subjects and senders", async () => {
  await app.fill(".search-input", "Tomaten");
  await app.waitText(".list-title", /Suche/);
  await app.waitUntil(async () => (await subjects(app)).join() === "Gartenplan", 10000, "one hit for the body text");
  await openMessage(app, "Gartenplan");
  await app.shot("search-local");
  await app.fill(".search-input", "Jonas");
  await app.waitUntil(async () => (await subjects(app)).join() === "Kino", 10000, "the sender is found");
  await app.fill(".search-input", "Garten");
  await app.waitUntil(async () => (await subjects(app)).join() === "Gartenplan", 10000, "a word start is enough");
});

it("asks the server for what is not here", async () => {
  await app.fill(".search-input", "Zauberwort");
  await app.waitText(".empty", /Zauberwort/);
  await clickText(app, ".list-foot .btn", "Auf dem Server suchen");
  await app.waitUntil(async () => (await subjects(app)).join() === "Langes Protokoll", 20000, "found on the server");
  await app.shot("search-server");
});

it("clears the search", async () => {
  await app.click(".search-clear");
  await app.waitText(".list-title", /Posteingang/);
  assert.equal((await subjects(app)).length, 3);
});
