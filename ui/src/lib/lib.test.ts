import { describe, expect, it } from "vitest";
import { langFromLocale, setLang, t } from "./i18n";
import { listDate } from "./format";
import { frameCsp, frameDocument, openableLink } from "./mailframe";
import { prefixed, replyDraft, splitAddresses } from "./compose";
import { resolveTheme } from "./theme";
import type { MessageView } from "./types";

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
  const colors = { paper: "white", text: "black", link: "blue", quote: "gray", font: "sans-serif" };
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
      date: 1_792_135_800, snippet: "", seen: true, flagged: false, answered: false, has_attachments: false,
    },
    to: [{ name: "Maria", email: "maria@example.org" }, { name: null, email: "team@example.org" }],
    cc: [{ name: null, email: "JONAS@example.org" }],
    message_id: "1@x",
    has_body: true,
    text: "Hallo\n> alt",
    remote_allowed: false,
    body: { html: "", blocked_remote: 0 },
  };
  const account = { id: 1, provider: "imap" as const, name: "", email: "maria@example.org", sender_name: "Maria" };

  it("prefixes subjects once", () => {
    expect(prefixed("Re", "AW: Re: Angebot")).toBe("Re: Angebot");
    expect(prefixed("Fwd", "WG: Bericht")).toBe("Fwd: Bericht");
    expect(prefixed("Re", "Rechnung")).toBe("Re: Rechnung");
  });

  it("replies to the sender, to all without yourself, and quotes", () => {
    const r = replyDraft(message, "reply", account, "de");
    expect(r.to).toBe("Jonas <jonas@example.org>");
    expect(r.cc).toBe("");
    expect(r.subject).toBe("Re: Angebot");
    expect(r.body).toContain("schrieb Jonas:\n> Hallo\n>> alt");
    const all = replyDraft(message, "replyAll", account, "de");
    expect(all.cc).toBe("team@example.org");
    const fwd = replyDraft(message, "forward", account, "en");
    expect(fwd.to).toBe("");
    expect(fwd.body).toContain("Forwarded message");
  });

  it("splits address fields", () => {
    expect(splitAddresses('"Muster, Maria" <m@x.org>; b@x.org, ,')).toEqual(['"Muster, Maria" <m@x.org>', "b@x.org"]);
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
