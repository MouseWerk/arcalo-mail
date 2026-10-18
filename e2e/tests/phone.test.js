// The phone's screens measured, not only looked for: the desktop app in its touch layout
// (ARCALO_MAIL_TOUCH=1) at two phone sizes (360×800, 412×915, portrait), set up from the first
// start against a Gmail-like account
// (crates/mail-testserver/src/gmail.rs). Every main screen (setup, inbox, folders, message,
// compose, settings, calendar) is as wide as the screen with nothing wider and nothing cut off
// above it; spinners stay small and round; the compose
// button leaves the last row free at the end of the list; every inbox tab can be reached;
// writing has one bar on top with the send button and one row of tools above the keyboard;
// folder names keep their brackets. The system bars are phone-insets.test.js's (set by hand)
// and the Android emulator's (e2e/android, the real bars); the same checks run there. A screenshot of each screen is kept (e2e/screenshots/phone-*).

import { describe, test, after, before } from "node:test";
import assert from "node:assert/strict";
import { launch, guarded } from "../lib/harness.js";
import { startServer } from "../lib/server.js";
import { clickText, openMessage, openView } from "../lib/mail.js";
import * as geo from "../lib/geometry.js";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** No system bars here (phone-insets.test.js sets them by hand, e2e/android has real ones). */
const INSETS = { top: 0, bottom: 0, left: 0, right: 0 };
const TABS = ["people", "newsletters", "notifications", "receipts"];

for (const [W, H] of [
  [360, 800],
  [412, 915],
]) {
  describe(`phone ${W}×${H}`, () => {
    let app;
    let server;
    let fixture;
    const it = guarded(test, () => app);

    /** The spinner recorder (again after a reload). */
    const prepare = () => app.browser.execute(geo.recordSpinners);

    /** The layout checks of the screen shown now, and its screenshot. */
    const screen = async (name, opts) => {
      await sleep(400);
      await app.shot(`phone-${W}x${H}-${name}`);
      const m = await app.browser.execute(geo.measureLayout, INSETS);
      return geo.layoutProblems(m, opts).map((p) => `${name}: ${p}`);
    };

    const spinners = async () => geo.spinnerProblems(await app.browser.execute(geo.recordedSpinners));

    before(async () => {
      server = await startServer();
      fixture = await server.gmail(60);
      await server.slow({ headers: 300, oldBodies: 1500 });
      app = await launch({ width: W, height: H, env: { ARCALO_MAIL_TOUCH: "1", ...server.discoveryEnv() } });
      await prepare();
    });

    after(async () => {
      await app?.close();
      server?.stop();
    });

    it("sets up the account on a screen that fits", async () => {
      await app.waitFor(".setup-welcome");
      const problems = await screen("setup");
      await app.fill(".setup-fields input[type=email]", server.user);
      await app.fill(".setup-fields input[type=password]", server.password);
      await app.click(".setup-fields button[type=submit]");
      await app.waitFor(".setup-done", 30000);
      problems.push(...(await screen("setup-done")));
      await app.click(".setup-done .btn-primary");
      await app.waitUntil(async () => (await app.$$(".row")).length > 0, 30000, "the inbox");
      problems.push(...(await spinners()));
      assert.deepEqual(problems, []);
    });

    it("shows the inbox with every tab in reach and the last row clear of the compose button", async () => {
      await app.reload();
      await prepare();
      await app.waitUntil(async () => (await app.$$(".row:not(.row-placeholder)")).length >= 5, 30000, "rows");
      await app.waitUntil(async () => (await app.$$(".inbox-tab")).length >= TABS.length, 30000, "the tabs");
      const problems = await screen("inbox");
      problems.push(...geo.tabProblems(await app.browser.execute(geo.measureTabs), TABS));
      for (let i = 0; i < 20; i++) {
        const r = await app.browser.execute(geo.scrollListToEnd);
        await sleep(300);
        if (!r.moved) break;
      }
      await sleep(400);
      await app.shot(`phone-${W}x${H}-inbox-end`);
      problems.push(...geo.fabProblems(await app.browser.execute(geo.measureFab)));
      problems.push(...(await spinners()));
      assert.deepEqual(problems, []);
    });

    it("lists the folders by their whole names", async () => {
      await app.click(".folders-btn");
      await app.waitFor(".sidebar");
      await app.waitUntil(async () => (await app.$$(".account .folder-name")).length >= 8, 20000, "the folders");
      const problems = await screen("folders");
      problems.push(...geo.nameProblems(await app.browser.execute(geo.measureNames, ".account .folder-name"), ["[Notion]", "Inbox", "Belege"]));
      await clickText(app, ".folder-name", "Posteingang");
      await app.waitFor(".list-pane");
      assert.deepEqual(problems, []);
    });

    it("shows a message on a screen that fits", async () => {
      await app.browser.execute(geo.scrollListToStart);
      await openMessage(app, fixture.cid, 30000);
      const problems = await screen("message");
      problems.push(...(await spinners()));
      await app.browser.back();
      await app.waitFor(".list-pane");
      assert.deepEqual(problems, []);
    });

    it("writes with one bar on top and one row of tools above the keyboard", async () => {
      await app.click(".compose-fab");
      await app.waitFor(".compose");
      const problems = await screen("compose");
      problems.push(...geo.composeProblems(await app.browser.execute(geo.measureCompose)));
      // The keyboard: Android makes the window smaller (adjustResize).
      await app.click(".compose .editor");
      await app.browser.setWindowSize(W, H - 300);
      await sleep(600);
      await app.shot(`phone-${W}x${H}-compose-keyboard`);
      problems.push(...geo.composeProblems(await app.browser.execute(geo.measureCompose), { keyboard: true }).map((p) => `with the keyboard: ${p}`));
      await app.browser.setWindowSize(W, H);
      await sleep(300);
      await app.browser.back();
      await app.waitUntil(async () => (await app.$$(".compose")).length === 0, 8000, "compose closed");
      assert.deepEqual(problems, []);
    });

    it("opens the settings and the calendar on screens that fit", async () => {
      await app.click(".folders-btn");
      await app.click('.sidebar-head [aria-label="Einstellungen"]');
      await app.waitFor(".settings-nav");
      const problems = await screen("settings");
      await clickText(app, ".settings-nav-item", "Lesen");
      await app.waitFor(".settings-main");
      problems.push(...(await screen("settings-reading")));
      await app.browser.back();
      await app.browser.back();
      await app.waitUntil(async () => (await app.$$(".settings")).length === 0, 5000, "settings closed");
      await openView(app, "Kalender");
      await app.waitFor(".calv-head", 15000);
      problems.push(...(await screen("calendar")));
      problems.push(...(await spinners()));
      assert.deepEqual(problems, []);
    });
  });
}
