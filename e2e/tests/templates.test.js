// Templates and snippets: both made in the settings, a snippet expanded while writing with
// „;;“ and its shortcut, a template that fills subject and text with the recipient's name, the
// date and the signature, sent over SMTP.

import { test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { addAccount, startServer } from "../lib/server.js";
import { clickText, menuItem, openSettings } from "../lib/mail.js";

let app;
let server;
const it = guarded(test, () => app);

before(async () => {
  server = await startServer();
  app = await launch();
  const account = await addAccount(app, server);
  await app.invoke("update_account", { id: account.id, name: "", senderName: "Maria Muster", signature: "Maria Muster\nTel. 0123 456789" });
  await app.browser.refresh();
});

after(async () => {
  await app?.close();
  server?.stop();
});

async function edit({ name, trigger, subject, body }) {
  await app.waitFor(".template-editor");
  const inputs = await app.$$(".template-editor .input:not(textarea)");
  const set = async (el, v) =>
    app.browser.execute(
      (e, value) => {
        const proto = e.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, "value").set.call(e, value);
        e.dispatchEvent(new Event("input", { bubbles: true }));
      },
      el,
      v,
    );
  await set(inputs[0], name);
  if (trigger) await set(await app.$(".template-editor .input-affix .input"), trigger);
  if (subject) {
    for (const l of await app.$$(".template-editor .stack-field")) {
      if ((await app.textOf(l)).startsWith("Betreff")) await set(await l.$(".input"), subject);
    }
  }
  await set(await app.$(".template-editor textarea"), body);
  await clickText(app, ".dialog-foot .btn-primary", "Speichern");
  await app.waitUntil(async () => (await app.$$(".template-editor")).length === 0, 5000, "editor closed");
}

async function bodyText() {
  return app.browser.execute(() => {
    const ed = document.querySelector(".compose .editor, .compose .compose-body");
    return ed.value ?? ed.innerText;
  });
}

it("makes a snippet and a template in the settings", async () => {
  await openSettings(app, "Vorlagen");
  await clickText(app, ".set-actions .btn span", "Textbaustein hinzufügen");
  await edit({ name: "Dank", trigger: "danke", body: "Vielen Dank, {vorname}!" });
  await clickText(app, ".set-actions .btn span", "Vorlage hinzufügen");
  await edit({ name: "Angebot", subject: "Angebot für {name}", body: "Hallo {vorname},\n\nanbei das Angebot vom {datum}.\n\n{signatur}" });
  await app.waitText(".template-list .rule-name", /;;danke\s+Dank/);
  await app.waitText(".template-list .rule-name", /Angebot/);
  await app.shot("templates-settings");
  await app.click('.settings-head [aria-label="Schließen"]');
  await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
});

it("expands a snippet typed with ;; and its shortcut", async () => {
  await (await app.$(".list-title")).click();
  await app.keys("c");
  await app.waitFor(".compose");
  await app.click(".compose .address-field .chip-input");
  await app.type("Lea Wagner <lea@example.org>");
  await app.keys("Enter");
  await app.browser.execute(() => document.querySelector(".compose .editor, .compose .compose-body").focus());
  await app.type(";;dan");
  await app.waitText(".snippet-pick .suggestion", /;;danke\s*Dank/);
  await app.shot("templates-snippet");
  await app.keys("Enter");
  await app.waitUntil(async () => /Vielen Dank, Lea!/.test(await bodyText()), 5000, "the snippet in the text");
  assert.doesNotMatch(await bodyText(), /;;/);
  assert.equal((await app.$$(".snippet-pick")).length, 0);
});

it("fills a template with name, date and signature", async () => {
  await app.click('[data-compose="templates"]');
  await menuItem(app, "Angebot");
  await app.waitUntil(async () => /Hallo Lea,/.test(await bodyText()), 5000, "the template in the text");
  const text = await bodyText();
  const d = new Date();
  assert.match(text, new RegExp(`anbei das Angebot vom ${d.getDate()}\\.`));
  assert.equal(text.match(/Tel\. 0123 456789/g)?.length, 1, "the signature once");
  assert.match(text, /Vielen Dank, Lea!/, "what was written stays below");
  assert.equal(await (await app.$("#compose-subject")).getValue(), "Angebot für Lea Wagner");
  await clickText(app, ".compose-foot .btn-primary", "Senden");
  await app.waitUntil(async () => (await server.sent()).length === 1, 20000, "sent");
  const raw = await server.sentRaw(0);
  assert.match(raw, /Hallo Lea,/);
  assert.match(raw, /Subject: =\?utf-8\?Q\?Angebot_f=C3=BCr_Lea_Wagner\?=/);
});
