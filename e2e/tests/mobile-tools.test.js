// The new tools on a phone and a tablet (the touch layout): the assistant above a message and
// its summary, the attachments with their buttons for a finger, questions to the mailbox, the
// contact card and the rule editor, all without sideways scrolling.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import { clickText, openMessage, openView, waitRow } from "../lib/mail.js";
import { pdf } from "../lib/png.js";

let app;
let server;
const it = guarded(test, () => app);
const WIDTH = 390;

before(async () => {
  server = await startServer();
  await server.deliver(
    message({
      from: "Lea Wagner <lea@example.org>",
      subject: "Angebot Website",
      body: "Hallo Maria,\n\nanbei das Angebot. Kannst du bis Freitag sagen, ob es passt?\n\nLea",
      attachments: [{ name: "angebot-website.pdf", type: "application/pdf", data: pdf("Angebot") }],
    }),
  );
  app = await launch({ width: WIDTH, height: 844, env: { ARCALO_MAIL_TOUCH: "1" } });
  await addAccount(app, server);
  const settings = await app.invoke("get_settings");
  settings.ai.providers = [{ id: "local", name: "Testmodell", kind: "compatible", base_url: `http://127.0.0.1:${server.control}/ai/v1`, model: "llama3.2", api_version: "" }];
  settings.ai.default_provider = "local";
  await app.invoke("set_settings", { settings });
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready");
  await waitRow(app, "Angebot Website");
});

after(async () => {
  await app?.close();
  server?.stop();
});

const fits = async (what) => {
  const w = await app.browser.execute(() => document.documentElement.scrollWidth);
  assert.ok(w <= WIDTH, `${what}: ${w} px wide`);
};

const rect = (sel) => app.browser.execute((s) => document.querySelector(s)?.getBoundingClientRect().toJSON() ?? null, sel);

it("summarizes on a phone", async () => {
  await openMessage(app, "Angebot Website");
  await app.waitFor(".assistant-bar");
  await app.click('[data-ai="summary"]');
  await app.waitText(".assistant-out .ai-text", /Offen: deine Antwort/, 15000);
  const b = await rect('[data-ai="summary"]');
  assert.ok(b.height >= 44, `a finger's target: ${JSON.stringify(b)}`);
  await fits("the message with the assistant");
  await app.shot("mobile-ai-summary");
});

it("opens the contact card on a phone", async () => {
  await app.click(".message-card .avatar-btn");
  await app.waitText(".contact-card .contact-name", /Lea Wagner/);
  const card = await rect(".contact-card");
  assert.ok(card.left >= 0 && card.right <= WIDTH, JSON.stringify(card));
  await app.waitText(".contact-card .contact-file", /angebot-website\.pdf/);
  await fits("the contact card");
  await app.click('.contact-card [aria-label="Schließen"]');
  await app.waitUntil(async () => (await app.$$(".contact-card")).length === 0, 5000, "card closed");
  await app.browser.back();
});

it("lists the attachments with buttons for a finger", async () => {
  await app.click(".folders-btn");
  await openView(app, "Anhänge");
  await app.waitText(".hub-item .hub-name", /angebot-website\.pdf/, 15000);
  const sizes = await app.browser.execute(() =>
    [...document.querySelectorAll(".hub-item .icon-btn, .hub-item .hub-main")].map((b) => {
      const r = b.getBoundingClientRect();
      return [b.getAttribute("aria-label") ?? b.className, r.height, getComputedStyle(b.parentElement).opacity];
    }),
  );
  for (const [label, h, o] of sizes) assert.ok(h >= 44 && o === "1", `${label}: ${h} px, opacity ${o}`);
  await fits("the attachments");
  await app.shot("mobile-attachments");
  await app.click('.hub-item [title="Nachricht öffnen"]');
  await app.waitText(".message-subject", /Angebot Website/);
  await app.browser.back();
});

it("asks the mailbox on a phone", async () => {
  await app.click(".folders-btn");
  await openView(app, "Postfach fragen");
  await app.fill(".ask-input", "Was steht im Angebot?");
  await app.click(".ask-send");
  await app.waitText(".ask-answer .ai-text", /Laut der Mail/, 15000);
  await fits("the answer");
});

it("edits a rule on a phone", async () => {
  await app.click(".folders-btn");
  await app.click('.sidebar-head [aria-label="Einstellungen"]');
  await app.waitFor(".settings-nav");
  await clickText(app, ".settings-nav-item", "Regeln");
  await clickText(app, ".set-actions .btn span", "Regel hinzufügen");
  await app.waitFor(".rule-editor");
  const editor = await rect(".rule-editor");
  assert.ok(editor.left >= 0 && editor.right <= WIDTH, JSON.stringify(editor));
  const overflow = await app.browser.execute(() => {
    const d = document.querySelector(".rule-editor .dialog-body");
    return d.scrollWidth - d.clientWidth;
  });
  assert.ok(overflow <= 1, `the editor scrolls sideways by ${overflow} px`);
  await app.shot("mobile-rule-editor");
});

it("shows the attachments next to the message on a tablet", async () => {
  await app.browser.setWindowSize(1024, 768);
  await app.browser.refresh();
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready");
  const nav = await app.$$(".folders-btn");
  if (nav.length && (await nav[0].isDisplayed())) await nav[0].click();
  await openView(app, "Anhänge");
  await app.waitText(".hub-item .hub-name", /angebot-website\.pdf/, 15000);
  await app.click('.hub-item [title="Nachricht öffnen"]');
  await app.waitText(".message-subject", /Angebot Website/);
  const list = await rect(".hub-pane");
  const pane = await rect(".reading-pane");
  assert.ok(list && pane && list.right <= pane.left + 1, `side by side: ${JSON.stringify({ list, pane })}`);
  await app.shot("tablet-attachments");
});
