import { describe, expect, it } from "vitest";
import { PULL_MAX, backStep, layoutFor, pullDistance, pullTriggers, swipeIntent, swipeOffset, swipeRemoves, swipeResult, swipeSide, swipeThreshold, typeScale } from "./touch";
import { langFromLocale, setLang, t } from "./i18n";
import { fileSize, initials, listDate } from "./format";
import { frameCsp, frameDocument, openableLink } from "./mailframe";
import { emptyDraft, isEmpty, isValidAddress, prefixed, replyDraft, splitAddresses, toDraft } from "./compose";
import { clickSelect, keyAction, pagesFor, visibleRange } from "./listing";
import { explain, hintKey } from "./problems";
import { score } from "./palette";
import { resolveTheme } from "./theme";
import type { MessageView } from "./types";
import { fromLocalInput, sendPresets, snoozePresets, toLocalInput, whenLabel } from "./when";

describe("i18n", () => {
  it("translates with values and plurals", () => {
    expect(t("list.count", { n: 1 })).toBe("1 Nachricht");
    expect(t("list.count", { n: 3 })).toBe("3 Nachrichten");
    expect(t("list.noResults", { q: "Rechnung" })).toBe("Nichts gefunden für „Rechnung“.");
    setLang("en");
    expect(t("list.count", { n: 1 })).toBe("1 message");
    expect(t("read.remoteBlocked", { n: 1 })).toMatch(/^One remote image/);
  });

  it("reads locale tags", () => {
    expect(langFromLocale("de-AT")).toBe("de");
    expect(langFromLocale("de_CH.UTF-8")).toBe("de");
    expect(langFromLocale("en-US")).toBe("en");
    expect(langFromLocale("dev")).toBe("en");
  });
});

describe("dates in the list", () => {
  const now = new Date(2026, 9, 16, 15, 0);
  const at = (y: number, mo: number, d: number, h = 9) => new Date(y, mo, d, h, 30).getTime() / 1000;
  it("shows time today, „Gestern“, day and month this year, the full date before", () => {
    expect(listDate(at(2026, 9, 16), now, "de")).toBe("09:30");
    expect(listDate(at(2026, 9, 15, 23), now, "de")).toBe("Gestern");
    expect(listDate(at(2026, 2, 3), now, "de")).toBe("3. März");
    expect(listDate(at(2025, 11, 24), now, "de")).toBe("24.12.2025");
  });
  it("knows yesterday across a clock change", () => {
    // 26.10.2026 is the Monday after the change to winter time in Germany.
    expect(listDate(at(2026, 9, 25, 1), new Date(2026, 9, 26, 0, 30), "de")).toBe("Gestern");
  });
});

describe("mail frame", () => {
  const colors = { paper: "white", text: "black", link: "blue", quote: "gray", rule: "silver", font: "sans-serif", pad: "0", scheme: "light" };
  it("blocks remote images unless they are allowed", () => {
    expect(frameCsp(false)).toContain("img-src data: cid:;");
    expect(frameCsp(false)).not.toContain("https:");
    expect(frameCsp(true)).toContain("https:");
    expect(frameCsp(true)).toContain("default-src 'none'");
    const doc = frameDocument("<p>x</p>", false, colors);
    expect(doc).toContain('http-equiv="Content-Security-Policy"');
    expect(doc).toContain("no-referrer");
  });
  it("opens web and mail links only", () => {
    expect(openableLink("https://example.org")).toBe("https://example.org");
    expect(openableLink("mailto:a@example.org")).toBe("mailto:a@example.org");
    expect(openableLink("javascript:alert(1)")).toBeNull();
    expect(openableLink("file:///etc/passwd")).toBeNull();
    expect(openableLink(null)).toBeNull();
  });
});

describe("compose", () => {
  const message: MessageView = {
    summary: {
      id: 1, mailbox_id: 1, uid: 1, thread_id: 1, subject: "AW: Re: Angebot", from: { name: "Jonas", email: "jonas@example.org" },
      date: 1_792_135_800, snippet: "", seen: true, flagged: false, answered: false, has_attachments: false, thread_count: 1, account_id: 1,
    },
    account_id: 1,
    to: [{ name: "Maria", email: "maria@example.org" }, { name: null, email: "team@example.org" }],
    cc: [{ name: null, email: "JONAS@example.org" }],
    reply_to: [],
    message_id: "1@x",
    in_reply_to: null,
    references: ["0@x"],
    size: 100,
    draft: false,
    has_body: true,
    text: "Hallo\n> alt",
    remote_allowed: false,
    body: { html: "", full_html: null, text_html: null, blocked_remote: 0, trackers: 0, plain: true },
    sender_trusted: false,
    files: [],
    load_error: null,
    unsubscribe: null,
    list_id: null,
    one_click: false,
    marks: { pinned: false, later: false, snoozed_until: null, followup_due: null },
    unsubscribed: null,
    protection: null,
    protected_files: [],
    invitation: null,
    meeting: null,
  };
  const account = { id: 1, provider: "imap" as const, name: "", email: "maria@example.org", sender_name: "Maria", transport: { kind: "graph" as const }, signature: "Maria Muster" };

  it("prefixes subjects once", () => {
    expect(prefixed("Re", "AW: Re: Angebot")).toBe("Re: Angebot");
    expect(prefixed("Fwd", "WG: Bericht")).toBe("Fwd: Bericht");
    expect(prefixed("Re", "Rechnung")).toBe("Re: Rechnung");
  });

  it("replies to the sender, to all without yourself, and quotes", () => {
    const r = replyDraft(message, "reply", account, "de", false);
    expect(r.to).toEqual(["Jonas <jonas@example.org>"]);
    expect(r.cc).toEqual([]);
    expect(r.subject).toBe("Re: Angebot");
    expect(r.text).toContain("schrieb Jonas:\n> Hallo\n>> alt");
    expect(r.text.indexOf("-- \nMaria Muster")).toBeLessThan(r.text.indexOf("schrieb"));
    expect(r.inReplyTo).toBe("1@x");
    expect(r.references).toEqual(["0@x", "1@x"]);
    expect(toDraft(r).reply_to_id).toBe(1);
    const all = replyDraft(message, "replyAll", account, "de", false);
    expect(all.cc).toEqual(["team@example.org"]);
    const fwd = replyDraft(message, "forward", account, "en", false);
    expect(fwd.to).toEqual([]);
    expect(fwd.text).toContain("Forwarded message");
    const rich = replyDraft({ ...message, reply_to: [{ name: "Liste", email: "list@example.org" }] }, "reply", account, "de", true);
    expect(rich.to).toEqual(["Liste <list@example.org>"]);
    expect(rich.html).toContain('<blockquote type="cite"><p>Hallo</p><p>&gt; alt</p></blockquote>');
    expect(toDraft(rich).html).toContain("blockquote");
    expect(toDraft(rich).text).toBe("");
  });

  it("knows empty drafts and valid addresses", () => {
    expect(isEmpty(emptyDraft(account, false))).toBe(true);
    expect(isEmpty({ ...emptyDraft(account, false), subject: "Hallo" })).toBe(false);
    expect(isValidAddress("Jonas <jonas@example.org>")).toBe(true);
    expect(isValidAddress("jonas@example")).toBe(false);
    expect(isValidAddress("jonas example.org")).toBe(false);
  });

  it("splits address fields", () => {
    expect(splitAddresses('"Muster, Maria" <m@x.org>; b@x.org, ,')).toEqual(['"Muster, Maria" <m@x.org>', "b@x.org"]);
  });
});

describe("the message list", () => {
  it("renders the rows on screen and loads their pages", () => {
    expect(visibleRange(0, 760, 76, 1000, 0)).toEqual({ start: 0, end: 11 });
    expect(visibleRange(7600, 760, 76, 1000)).toEqual({ start: 92, end: 119 });
    expect(visibleRange(0, 760, 76, 0)).toEqual({ start: 0, end: 0 });
    expect(pagesFor(90, 130, 100)).toEqual([0, 1]);
  });

  it("selects with Shift and Ctrl", () => {
    const order = [10, 11, 12, 13];
    let s = clickSelect({ ids: [], anchor: null }, order, 1, { shift: false, toggle: false });
    expect(s).toEqual({ ids: [11], anchor: 11 });
    s = clickSelect(s, order, 3, { shift: true, toggle: false });
    expect(s.ids.sort()).toEqual([11, 12, 13]);
    s = clickSelect(s, order, 2, { shift: false, toggle: true });
    expect(s.ids.sort()).toEqual([11, 13]);
  });

  it("maps keys to actions, not while typing", () => {
    const k = (key: string, extra: Partial<KeyboardEvent> = {}) => ({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...extra });
    expect(keyAction(k("j"), false)).toBe("next");
    expect(keyAction(k("#"), false)).toBe("delete");
    expect(keyAction(k("e"), true)).toBeNull();
    expect(keyAction(k("Escape"), true)).toBe("escape");
    expect(keyAction(k("n", { ctrlKey: true }), true)).toBe("compose");
    expect(keyAction(k("U", { shiftKey: true }), false)).toBe("markUnread");
  });
});

describe("problems and sizes", () => {
  it("explain what to do", () => {
    expect(explain({ kind: "auth", detail: "no" }).title).toBe("problem.auth");
    const c = explain({ kind: "certificate", host: "mail.example.org", fingerprint: "AA", detail: "self-signed" });
    expect(t(c.hint, c.vars)).toContain("mail.example.org");
    expect(hintKey("enable_imap")).toBe("setup.hintEnableImap");
  });

  it("formats sizes and initials", () => {
    expect(fileSize(512, "de")).toBe("512 B");
    expect(fileSize(1_258_291, "de")).toBe("1,2 MB");
    expect(fileSize(1_258_291, "en")).toBe("1.2 MB");
    expect(initials({ name: "Jonas Berger", email: "j@x.org" })).toBe("JB");
    expect(initials({ name: null, email: "lea@x.org" })).toBe("LE");
  });
});

describe("theme", () => {
  it("follows the system unless chosen", () => {
    expect(resolveTheme("system", true, false)).toBe("dark");
    expect(resolveTheme("system", false, false)).toBe("light");
    expect(resolveTheme("system", false, true)).toBe("contrast");
    expect(resolveTheme("light", true, true)).toBe("light");
  });
});

describe("milestone 2", () => {
  it("opens the command palette with Ctrl or Cmd+K, also while typing", () => {
    const key = (k: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean }> = {}) => ({ key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
    expect(keyAction(key("k", { ctrlKey: true }), true)).toBe("palette");
    expect(keyAction(key("K", { metaKey: true }), false)).toBe("palette");
    expect(keyAction(key("k"), false)).toBe("prev");
  });

  it("ranks palette commands by word starts", () => {
    expect(score("Gehe zu Posteingang", "post")).toBeGreaterThanOrEqual(0);
    expect(score("Gehe zu Posteingang", "eingang")).toBeGreaterThan(score("Gehe zu Posteingang", "post"));
    expect(score("Neue Nachricht schreiben", "neu nach")).toBeGreaterThanOrEqual(0);
    expect(score("Neue Nachricht schreiben", "xyz")).toBe(-1);
    expect(score("anything", "")).toBe(0);
  });

  it("explains the Microsoft and proxy problems", () => {
    expect(explain({ kind: "admin_consent", detail: "AADSTS65001" }).title).toBe("problem.adminConsent");
    expect(explain({ kind: "smtp_auth_disabled", detail: "535 5.7.139" }).hint).toBe("problem.smtpAuthDisabledHint");
    const p = explain({ kind: "proxy", proxy: "http://proxy:3128", detail: "403" });
    expect([p.title, p.vars.proxy, p.detail]).toEqual(["problem.proxy", "http://proxy:3128", "403"]);
    expect(hintKey("google_app_password")).toBeNull();
  });
});

describe("touch", () => {
  it("chooses one pane on phones, a drawer on tablets, three panes elsewhere", () => {
    expect(layoutFor(390, true)).toBe("phone");
    expect(layoutFor(390, false)).toBe("phone");
    expect(layoutFor(820, true)).toBe("tablet");
    expect(layoutFor(844, true)).toBe("tablet");
    expect(layoutFor(1180, true)).toBe("desktop");
    expect(layoutFor(1000, false)).toBe("desktop");
  });

  it("tells a swipe from scrolling and picks the side's action", () => {
    expect(swipeIntent(4, 3)).toBeNull();
    expect(swipeIntent(30, 5)).toBe("swipe");
    expect(swipeIntent(-30, 8)).toBe("swipe");
    expect(swipeIntent(12, 30)).toBe("scroll");
    expect(swipeSide(-40, "archive", "read")).toBe("archive");
    expect(swipeSide(40, "archive", "read")).toBe("read");
    expect(swipeSide(40, "archive", "none")).toBeNull();
  });

  it("acts only past the threshold, and holds back past it", () => {
    const w = 390;
    expect(swipeThreshold(w)).toBe(96);
    expect(swipeThreshold(150)).toBe(48);
    expect(swipeResult(-60, w, "archive", "read")).toBeNull();
    expect(swipeResult(-120, w, "archive", "read")).toBe("archive");
    expect(swipeResult(120, w, "archive", "none")).toBeNull();
    expect(swipeOffset(-50, w, "archive", "read")).toBe(-50);
    expect(Math.abs(swipeOffset(-200, w, "archive", "read"))).toBeLessThan(200);
    expect(Math.abs(swipeOffset(200, w, "archive", "none"))).toBeLessThanOrEqual(6);
    expect(swipeRemoves("archive") && swipeRemoves("delete")).toBe(true);
    expect(swipeRemoves("read") || swipeRemoves("flag")).toBe(false);
  });

  it("pulls with resistance and refreshes past the trigger", () => {
    expect(pullDistance(-20)).toBe(0);
    expect(pullDistance(100)).toBe(50);
    expect(pullDistance(1000)).toBe(PULL_MAX);
    expect(pullTriggers(100)).toBe(false);
    expect(pullTriggers(140)).toBe(true);
  });

  it("follows the iOS text size within bounds", () => {
    expect(typeScale(17)).toBe(1);
    expect(typeScale(23)).toBe(1.35);
    expect(typeScale(53)).toBe(1.5);
    expect(typeScale(12)).toBe(0.88);
    expect(typeScale(null)).toBe(1);
    expect(typeScale(Number.NaN)).toBe(1);
  });

  it("walks back through compose, settings, the message and the folders", () => {
    const s = { view: "message" as const, layout: "phone" as const, composing: false, settings: false };
    expect(backStep({ ...s, composing: true })).toBe("compose");
    expect(backStep({ ...s, settings: true })).toBe("settings");
    expect(backStep(s)).toBe("list");
    expect(backStep({ ...s, layout: "tablet" })).toBeNull();
    expect(backStep({ ...s, view: "folders", layout: "tablet" })).toBe("list");
    expect(backStep({ ...s, view: "list" })).toBeNull();
  });
});

describe("the time tools' times", () => {
  // Friday 16 October 2026, 15:20.
  const fri = new Date(2026, 9, 16, 15, 20);
  const at = (y: number, mo: number, d: number, h: number, mi = 0) => new Date(y, mo, d, h, mi).getTime() / 1000;

  it("offers later today, tomorrow and next week, the weekend from Monday to Thursday", () => {
    expect(snoozePresets(fri)).toEqual([
      { id: "laterToday", at: at(2026, 9, 16, 19) },
      { id: "tomorrow", at: at(2026, 9, 17, 8) },
      { id: "nextWeek", at: at(2026, 9, 19, 8) },
    ]);
    const tue = snoozePresets(new Date(2026, 9, 13, 9, 0)).map((p) => p.id);
    expect(tue).toEqual(["laterToday", "tomorrow", "weekend", "nextWeek"]);
    expect(snoozePresets(new Date(2026, 9, 16, 21, 30)).map((p) => p.id)).not.toContain("laterToday");
    // A Sunday's next week is the Monday after tomorrow's.
    expect(snoozePresets(new Date(2026, 9, 18, 10, 0)).find((p) => p.id === "nextWeek")?.at).toBe(at(2026, 9, 26, 8));
    expect(sendPresets(fri).map((p) => p.at)).toEqual([at(2026, 9, 17, 8), at(2026, 9, 17, 13), at(2026, 9, 19, 8)]);
  });

  it("names near times and reads the time field", () => {
    expect(whenLabel(at(2026, 9, 16, 19), fri, "de")).toBe("Heute, 19:00");
    expect(whenLabel(at(2026, 9, 17, 8), fri, "en")).toBe("Tomorrow, 08:00");
    // Weekday and month names as the platform's ICU writes them („Mo“ or „Mo.“).
    expect(whenLabel(at(2026, 9, 19, 8), fri, "de")).toMatch(/^Mo\.?, 08:00$/);
    expect(whenLabel(at(2026, 11, 24, 18, 30), fri, "de")).toMatch(/^24\. Dez\.?, 18:30$/);
    expect(fromLocalInput(toLocalInput(at(2026, 9, 17, 8)))).toBe(at(2026, 9, 17, 8));
    expect(fromLocalInput("2026-10-17T08:00:30")).toBe(at(2026, 9, 17, 8) + 30);
    expect(fromLocalInput("")).toBeNull();
  });

  it("has keys for snooze, pin and more", () => {
    const k = (key: string) => keyAction({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false }, false);
    expect([k("h"), k("p"), k(".")]).toEqual(["snooze", "pin", "more"]);
  });
});
