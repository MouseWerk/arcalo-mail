// A tour of every screen of the phone and tablet layouts (not part of the test suite), for the
// design review: the desktop app in its touch layout (ARCALO_MAIL_TOUCH=1) with the sample
// mailboxes of the README pictures on the fixed clock, with the system bars of an Android phone
// simulated (--sat/--sab, as MainActivity.kt sets them). Run after building the app and the test
// server:
//   cd e2e && node --test shots/phone-tour.test.js
// TOUR_SIZE (default 412x915), TOUR_THEMES (default light,dark,contrast), TOUR_LOCALE (default
// en-GB), TOUR_INSETS (default 48,0,24,0: top, right, bottom, left) and ARCALO_MAIL_SHOTS (the
// folder, default e2e/screenshots/tour) choose what is drawn. A step that cannot be reached is
// noted and skipped, so a tour always finishes.

import { test, before, after } from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launch } from "../lib/harness.js";
import { addAccount, message, startServer } from "../lib/server.js";
import * as mail from "../lib/mail.js";
import { NOW, PERSONAL, WORK, fill, personalMail, workMail } from "./mailbox.js";

const OUT = process.env.ARCALO_MAIL_SHOTS ?? path.resolve(import.meta.dirname, "../screenshots/tour");
const [W, H] = (process.env.TOUR_SIZE ?? "412x915").split("x").map(Number);
const THEMES = (process.env.TOUR_THEMES ?? "light,dark,contrast").split(",");
const LOCALE = process.env.TOUR_LOCALE ?? "en-GB";
const INSETS = (process.env.TOUR_INSETS ?? "48,0,24,0").split(",").map(Number);
/** The system's font size as Android hands it over (130 % is TOUR_SCALE=1.3). */
const SCALE = process.env.TOUR_SCALE ?? "";
const DE = LOCALE.startsWith("de");
const L = DE
  ? { inbox: "Posteingang", calendar: "Kalender", attachments: "Anhänge", settings: "Einstellungen", reply: "Antworten", more: "Weitere Aktionen", close: "Schließen und Entwurf behalten" }
  : { inbox: "Inbox", calendar: "Calendar", attachments: "Attachments", settings: "Settings", reply: "Reply", more: "More actions", close: "Close and keep the draft" };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
for (const k of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
  delete process.env[k];
  delete process.env[k.toLowerCase()];
}

let work, personal, app, dataDir, theme;
const prefix = () => `${W}x${H}-${LOCALE.slice(0, 2)}${SCALE ? `-${SCALE}x` : ""}-${theme}`;

/** The simulated system bars: what MainActivity.kt sets on a phone, in CSS pixels. */
async function insets() {
  await app.browser.execute(([t, r, b, l, scale]) => {
    const s = document.documentElement.style;
    s.setProperty("--sat", `${t}px`);
    s.setProperty("--sar", `${r}px`);
    s.setProperty("--sab", `${b}px`);
    s.setProperty("--sal", `${l}px`);
    if (scale) s.setProperty("--type-scale", scale);
    // As SystemBars.kt does after a change.
    window.dispatchEvent(new Event("arcalo-insets"));
  }, [...INSETS, SCALE]);
}

async function shot(name, { blur = true } = {}) {
  await app.browser.execute(() => document.querySelectorAll(".toast .icon-btn").forEach((b) => b.click()));
  if (blur) await app.browser.execute(() => document.activeElement?.blur());
  await insets();
  await sleep(450);
  await app.shot(`${prefix()}-${name}`, OUT);
}

async function ready() {
  await app.waitUntil(async () => (await app.browser.execute(() => document.body.classList.contains("ready"))) === true, 20000, "ready after reload");
  await insets();
}

async function setTheme(t) {
  theme = t;
  const s = await app.invoke("get_settings");
  await app.invoke("set_settings", { settings: { ...s, theme: t } });
  await app.browser.refresh();
  await ready();
}

/** One step of the tour; a failure is noted and the tour goes on from the inbox. */
async function step(name, fn) {
  try {
    await fn();
  } catch (e) {
    console.log(`tour: ${prefix()} ${name}: ${String(e?.message ?? e).split("\n")[0]}`);
    await app.browser.keys("Escape").catch(() => {});
    await app.browser.refresh().catch(() => {});
    await ready().catch(() => {});
  }
}

/** A tap beside an open menu sheet closes it. */
async function closeSheet() {
  await app.browser.execute(() => document.querySelector(".sheet-scrim")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })));
  await sleep(300);
}

/** Closes the compose sheet (the draft is kept). */
async function closeCompose() {
  if (await app.browser.execute(() => !!document.querySelector(".compose"))) {
    await app.click(`.compose-head [aria-label="${L.close}"]`);
    await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
  }
}

async function inbox() {
  await closeCompose();
  for (let i = 0; i < 3; i++) {
    const view = await app.browser.execute(() => document.querySelector(".app")?.dataset.view);
    if (!view || view === "list") break;
    await app.browser.back();
    await sleep(300);
  }
  if (await app.browser.execute(() => !!document.querySelector(".folders-btn"))) {
    await app.click(".folders-btn");
    await mail.openFolder(app, "Work", L.inbox);
  }
  await app.waitFor(".list-pane");
  if (await app.browser.execute(() => document.querySelector(".inbox-tab[aria-selected='true']")?.dataset.tab !== "people" && !!document.querySelector(".inbox-tab"))) {
    await mail.openTab(app, "people");
  }
}

async function drawer() {
  await inbox();
  await app.click(".folders-btn");
  await app.waitFor(".sidebar");
}

before(async () => {
  work = await startServer({ user: WORK, password: "sample-password" });
  personal = await startServer({ user: PERSONAL, password: "sample-password" });
  await fill(work, workMail());
  await fill(personal, personalMail());
  await work.deliver(
    message({ from: "Daniel Okafor <daniel@example.com>", to: WORK, subject: "Encrypted: contract draft", body: "-----BEGIN PGP MESSAGE-----\n\nhQEMA1234567890ABAQf+\n=abcd\n-----END PGP MESSAGE-----\n" }),
  );
  dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-tour-"));
  app = await launch({
    width: W,
    height: H,
    locale: LOCALE,
    dataDir,
    env: { ARCALO_MAIL_NOW: String(NOW), TZ: "Europe/London", ARCALO_MAIL_TOUCH: "1", ARCALO_MAIL_AUTOCONFIG: `http://127.0.0.1:${work.control}/autoconfig?emailaddress={email}` },
  });
  theme = "system";
});

after(async () => {
  await app?.close().catch(() => {});
  work?.stop();
  personal?.stop();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

test("setup", async () => {
  await insets();
  await app.waitFor(".setup");
  for (const t of THEMES) {
    theme = t;
    await app.browser.execute((x) => (document.documentElement.dataset.theme = x), t);
    await step("welcome", async () => {
      await app.browser.execute(() => document.querySelector(".setup")?.scrollTo?.(0, 0));
      await shot("setup-welcome");
      await app.browser.execute(() => {
        const s = document.querySelector(".setup");
        s?.scrollTo?.(0, s.scrollHeight);
      });
      await shot("setup-welcome-end");
    });
  }
  theme = THEMES[0];
  await app.browser.execute((x) => (document.documentElement.dataset.theme = x), theme);
  await step("gmail", async () => {
    await app.fill(".setup-fields input[type=email]", "name@gmail.com");
    await app.waitFor(".guide");
    await shot("setup-gmail");
  });
  await step("keyboard", async () => {
    // The keyboard open: the window is smaller by its height, as on Android (adjustResize).
    await app.browser.setWindowSize(W, Math.round(H * 0.58));
    await app.fill(".setup-fields input[type=email]", "name@example.org");
    await shot("setup-keyboard", { blur: false });
    await app.browser.setWindowSize(W, H);
  });
  await step("error", async () => {
    await app.fill(".setup-fields input[type=email]", WORK);
    await app.fill(".setup-fields input[type=password]", "wrong");
    await app.click(".setup-fields button[type=submit]");
    await app.waitFor(".problem", 20000);
    await shot("setup-error");
  });
  await step("manual", async () => {
    await app.click(".setup-fields .btn-quiet");
    await app.waitFor(".server-fields");
    await shot("setup-manual");
  });
  await addAccount(app, work, { name: "Work", sender: "Emma Larsen" });
  await addAccount(app, personal, { name: "Personal", sender: "Emma Larsen" });
  await app.browser.refresh();
  await ready();
  await app.waitUntil(async () => (await app.$$(".row")).length >= 6, 30000, "rows");
});

for (const t of THEMES) {
  test(`app, ${t}`, async () => {
    await setTheme(t);
    await step("inbox", async () => {
      await inbox();
      await app.waitUntil(async () => (await app.$$(".row-snippet")).length >= 4, 20000, "snippets");
      await shot("inbox");
      await app.browser.execute(() => {
        const l = document.querySelector(".messages");
        l?.scrollTo?.(0, l.scrollHeight);
      });
      await shot("inbox-end");
    });
    await step("drawer", async () => {
      await drawer();
      await shot("drawer");
      await app.click(".sidebar .folder");
    });
    await step("message-html", async () => {
      await inbox();
      // The newsletters of one sender are a bundle in the inbox; the search lists each message.
      await app.fill(".search-input", "Designing for slow software");
      await mail.openMessage(app, "Field Notes #48");
      await shot("message-html");
      await app.browser.back();
      await app.click(".search-clear");
    });
    await step("message-attachments", async () => {
      await inbox();
      await mail.openMessage(app, "Photos from the offsite");
      await shot("message-attachments");
      await app.browser.execute(() => {
        const s = document.querySelector(".reading-scroll, .reading-pane .scroll");
        s?.scrollTo?.(0, s.scrollHeight);
      });
      await shot("message-attachments-end");
    });
    await step("invitation", async () => {
      await inbox();
      await mail.openMessage(app, "Invitation: Design review");
      await shot("message-invitation");
    });
    await step("encrypted", async () => {
      await inbox();
      await mail.openMessage(app, "Encrypted: contract draft");
      await shot("message-encrypted");
    });
    await step("snooze", async () => {
      await inbox();
      await mail.openMessage(app, "Q2 launch plan");
      await app.click('[data-action="snooze"]');
      await app.waitFor(".menu, .sheet");
      await shot("snooze");
      await closeSheet();
    });
    await step("message-more", async () => {
      await inbox();
      await mail.openMessage(app, "Q2 launch plan");
      await app.click(`.reading-pane [aria-label="${L.more}"]`);
      await app.waitFor(".menu, .sheet");
      await shot("message-more");
      await closeSheet();
    });
    await step("compose", async () => {
      await inbox();
      await app.click(".compose-fab");
      await app.waitFor(".compose");
      await shot("compose");
      await app.browser.setWindowSize(W, Math.round(H * 0.58));
      await app.browser.execute(() => document.querySelector(".compose .editor")?.focus());
      await shot("compose-keyboard", { blur: false });
      await app.browser.setWindowSize(W, H);
      await app.browser.execute(() => document.activeElement?.blur());
      await app.click(".compose [data-compose='more']").catch(() => {});
      if (await app.browser.execute(() => !!document.querySelector(".menu"))) {
        await shot("compose-more");
        await closeSheet();
      }
      await closeCompose();
    });
    await step("reply", async () => {
      await inbox();
      await mail.openMessage(app, "Q2 launch plan");
      await app.click(`.touch-actions [aria-label="${L.reply}"]`);
      await app.waitFor(".compose");
      await shot("compose-reply");
      await closeCompose();
    });
    await step("search", async () => {
      await inbox();
      await app.fill(".search-input", "launch");
      await sleep(800);
      await shot("search", { blur: false });
      await app.click(".search-clear");
    });
    await step("tabs", async () => {
      await inbox();
      const tabs = await app.$$(".tabs .tab, .list-tabs button");
      if (tabs.length > 1) await tabs[tabs.length - 1].click();
      await shot("inbox-tab");
    });
    await step("calendar", async () => {
      await drawer();
      await mail.openView(app, L.calendar);
      await app.waitFor(".calv");
      await shot("calendar");
      const views = ["day", "agenda", "month"];
      for (const v of views) {
        await mail.choose(app, ".calv-view-select", v);
        await shot(`calendar-${v}`);
      }
    });
    await step("attachments-hub", async () => {
      await drawer();
      await mail.openView(app, L.attachments);
      await app.waitFor(".hub-item", 15000);
      await shot("attachments-hub");
    });
    await step("contact", async () => {
      await inbox();
      await mail.openMessage(app, "Q2 launch plan");
      await app.click(".message-card .avatar-btn");
      await app.waitFor(".contact-card");
      await shot("contact-card");
      await app.keys("Escape");
    });
    await step("settings", async () => {
      await drawer();
      await app.click(`.sidebar-head [aria-label="${L.settings}"]`);
      await app.waitFor(".settings-nav");
      await shot("settings");
      const n = (await app.$$(".settings-nav-item")).length;
      for (let i = 0; i < n; i++) {
        const items = await app.$$(".settings-nav-item");
        const label = (await app.textOf(items[i])).replace(/\W+/g, "-").toLowerCase();
        await items[i].click();
        await app.waitFor(".settings-main");
        await shot(`settings-${String(i).padStart(2, "0")}-${label}`);
        await app.browser.execute(() => {
          const m = document.querySelector(".settings-main");
          m?.scrollTo?.(0, m.scrollHeight);
        });
        await shot(`settings-${String(i).padStart(2, "0")}-${label}-end`);
        // A phone shows the sections as a page of their own; Back leads there.
        if (await app.browser.execute(() => document.querySelector(".app").dataset.layout === "phone")) {
          await app.browser.back();
          await app.waitFor(".settings-nav");
        }
      }
      await app.keys("Escape");
    });
    await step("rules", async () => {
      await drawer();
      await app.click(`.sidebar-head [aria-label="${L.settings}"]`);
      await app.waitFor(".settings-nav");
      await mail.clickText(app, ".settings-nav-item", DE ? "Regeln" : "Rules");
      await mail.clickText(app, ".set-actions .btn span", DE ? "Regel hinzufügen" : "Add a rule");
      await app.waitFor(".rule-editor");
      await shot("rules-editor");
      await app.keys("Escape");
      await app.keys("Escape");
    });
  });
}
