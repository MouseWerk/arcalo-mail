// The assistant against a fake OpenAI-compatible provider on this computer (the test server's
// `/ai/v1`): off until a provider is set up, a summary streamed into the reading pane, a reply
// draft that opens in the compose sheet, stopping an answer, a clear error, tasks and dates
// with a reminder and a calendar entry, writing help in the compose sheet, questions to the
// mailbox with links to the messages, and an account whose mail no provider may see.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, menuItem, openFolder, openMessage, openView } from "../lib/mail.js";

let app;
let server;
let other;
const it = guarded(test, () => app);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const requests = async () => (await fetch(`http://127.0.0.1:${server.control}/ai-requests`)).json();
const control = (p, body) => fetch(`http://127.0.0.1:${server.control}${p}`, { method: "POST", body }).then((r) => r.text());

before(async () => {
  server = await startServer();
  other = await startServer({ user: "ben@firma.example" });
  await server.deliver(
    message({
      from: "Lea Wagner <lea@example.org>",
      subject: "Angebot Website",
      body: "Hallo Maria,\n\nanbei das Angebot für die neue Website. Kannst du bis Freitag sagen, ob es passt?\n\nViele Grüße\nLea",
    }),
  );
  await other.deliver(message({ from: "Chef <chef@firma.example>", to: "Ben <ben@firma.example>", subject: "Vertraulich: Umbau", body: "Bitte noch nicht weitersagen." }));
  app = await launch();
  await addAccount(app, server);
  await addAccount(app, other, { sender: "Ben" });
  await app.browser.refresh();
});

async function discard() {
  await app.click('.compose-foot [aria-label="Verwerfen"]');
  await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 5000, "compose closed");
}

after(async () => {
  await app?.close();
  server?.stop();
  other?.stop();
});

it("stays off until a provider is set up", async () => {
  await openMessage(app, "Angebot Website");
  assert.equal((await app.$$(".assistant")).length, 0, "no assistant bar");
  assert.equal((await app.$$('.views [data-view="ask"]')).length, 0, "no ask view");
  assert.deepEqual(await requests(), []);
  // A provider on this computer for every account, but the second account keeps its mail.
  const accounts = await app.invoke("list_accounts");
  const ben = accounts.find((a) => a.email === "ben@firma.example");
  const settings = await app.invoke("get_settings");
  settings.ai.providers = [{ id: "local", name: "Testmodell", kind: "compatible", base_url: `http://127.0.0.1:${server.control}/ai/v1`, model: "llama3.2", api_version: "" }];
  settings.ai.default_provider = "local";
  settings.ai.accounts = { [String(ben.id)]: "off" };
  await app.invoke("set_settings", { settings });
  await app.browser.refresh();
});

it("summarizes a conversation, streamed, and says where the text goes", async () => {
  await openMessage(app, "Angebot Website");
  await app.waitFor(".assistant-bar");
  assert.match(await app.text(".assistant-bar .privacy-line"), /Bleibt auf diesem Gerät · Testmodell/);
  await app.click('[data-ai="summary"]');
  // While it streams, the caret shows; at the end the whole text is there.
  await app.waitFor(".assistant-out .ai-caret", 5000);
  await app.waitText(".assistant-out .ai-text", /Lea Wagner.*schreibt über „Angebot Website“[\s\S]*Offen: deine Antwort\./, 15000);
  await app.waitUntil(async () => (await app.$$(".assistant-out .ai-caret")).length === 0, 10000, "the stream ends");
  const sent = await requests();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].task, "summary");
  assert.equal(sent[0].model, "llama3.2");
  assert.match(sent[0].user, /Kannst du bis Freitag sagen/, "the mail text goes to the provider");
  assert.match(sent[0].system, /Deutsch/, "asked for in the user's language");
  await app.shot("ai-summary");
});

it("drafts a reply and opens it in the compose sheet", async () => {
  await app.click('[data-ai="reply"]');
  await app.fill(".assistant-notes .input", "zusagen");
  await clickText(app, ".assistant-notes .btn", "Entwerfen");
  await app.waitText(".assistant-out .ai-text", /danke für deine Nachricht/, 15000);
  const last = (await requests()).at(-1);
  assert.equal(last.task, "reply");
  assert.match(last.user, /zusagen/);
  await clickText(app, ".assistant-out-foot .btn-primary span", "Als Antwort öffnen");
  await app.waitFor(".compose");
  assert.equal(await (await app.$("#compose-subject")).getValue(), "Re: Angebot Website");
  const body = await app.browser.execute(() => document.querySelector(".compose .editor, .compose .compose-body")?.innerText ?? document.querySelector(".compose .compose-body")?.value);
  assert.match(body, /danke für deine Nachricht/);
  assert.match(body, /Lea/, "the quote stays below");
  await discard();
});

it("stops an answer and keeps what came", async () => {
  await control("/ai-answer?task=translate", "Dies ist eine lange Übersetzung. ".repeat(40));
  await app.click('[data-ai="translate"]');
  await app.waitText(".assistant-out .ai-text", /Dies ist eine/, 10000);
  await clickText(app, ".assistant-out-head .btn span", "Anhalten");
  await app.waitText(".assistant-out p", /Angehalten/, 5000);
  const kept = await app.text(".assistant-out .ai-text");
  await sleep(800);
  assert.equal(await app.text(".assistant-out .ai-text"), kept, "nothing more arrives");
  assert.ok(kept.length < 40 * 33, "cut short");
});

it("explains a failing provider", async () => {
  await control("/ai-fail?status=500");
  await app.click('[data-ai="extract"]');
  await app.waitText(".assistant-out .notice-warning", /fake provider failed on purpose|500/, 15000);
});

it("finds tasks and dates, sets a reminder and opens a calendar entry", async () => {
  await app.click('[data-ai="extract"]');
  await app.waitFor(".assistant-out .ai-item", 15000);
  const items = await app.$$(".assistant-out .ai-item");
  assert.equal(items.length, 2);
  assert.match(await app.textOf(items[0]), /Angebot Website/);
  assert.match(await app.textOf(items[1]), /Antworten: Angebot Website/);
  await clickText(app, ".assistant-out .ai-item:nth-child(2) .btn span", "Erinnern");
  await app.waitText(".toast-title", /Du wirst erinnert/);
  await app.waitText(".assistant-out .ai-item:nth-child(2) .btn span", /Erinnerung gestellt/);
  await clickText(app, ".assistant-out .ai-item:nth-child(1) .btn span", "In den Kalender");
  await app.waitText(".toast-title", /Kalender-App/);
  await app.waitUntil(async () => app.opened().some((l) => l.endsWith(".ics")), 5000, "the .ics went to the calendar app");
  await app.shot("ai-tasks");
});

it("helps with writing in the compose sheet", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor, .compose .compose-body");
    ed.focus();
  });
  await app.type("Hallo Jonas, kannst du mir dein Angebot schicken?");
  await app.click('[data-compose="help"]');
  await menuItem(app, "Förmlich (Sie-Form)");
  await app.waitText(".writing-help .ai-text", /Guten Tag Jonas, können Sie mir Ihr Angebot schicken\?/, 15000);
  const last = (await requests()).at(-1);
  assert.equal(last.task, "rewrite");
  assert.match(last.user, /kannst du mir dein Angebot/);
  await app.shot("ai-writing-help");
  await clickText(app, ".writing-help .btn-primary span", "Übernehmen");
  const body = await app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor, .compose .compose-body");
    return ed.value ?? ed.innerText;
  });
  assert.match(body, /können Sie mir Ihr Angebot/);
  await discard();
});

it("answers questions about the mail with links to the messages", async () => {
  await openView(app, "Postfach fragen");
  await app.waitFor(".ask-input");
  await app.fill(".ask-input", "Wann kommt das Angebot für die Website?");
  await app.click(".ask-send");
  await app.waitText(".ask-answer .ai-text", /Laut der Mail von .*Lea/, 15000);
  const last = (await requests()).at(-1);
  assert.equal(last.task, "ask");
  assert.match(last.user, /Angebot Website/);
  assert.doesNotMatch(last.user, /Vertraulich/, "the other account's mail stays out");
  await app.waitFor(".ask-sources .smart-item");
  await app.shot("ai-ask");
  await app.click(".ask-answer button.cite");
  await app.waitText(".message-subject", /Angebot Website/);
});

it("sends nothing for an account no provider may see", async () => {
  const before = (await requests()).length;
  await openFolder(app, "ben@firma.example", "Posteingang");
  await openMessage(app, "Vertraulich: Umbau");
  await app.waitText(".assistant-off", /Kein KI-Anbieter darf die Mails dieses Kontos sehen/);
  assert.equal((await app.$$('[data-ai="summary"]')).length, 0);
  // The account's destination says so too.
  const ben = (await app.invoke("list_accounts")).find((a) => a.email === "ben@firma.example");
  const dest = await app.invoke("ai_destination", { accountId: ben.id });
  assert.equal(dest.provider, null);
  await sleep(500);
  assert.equal((await requests()).length, before, "no request to the provider");
});
