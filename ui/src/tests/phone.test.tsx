// Phones: the Android shell's system bar insets and font size on the page, the theme handed back
// for the bars' icons, and the phone layout of the app itself: the inbox's tabs as chips with
// Focus, the sender's picture in the rows, writing with an app bar instead of a footer, menus as
// sheets from the bottom, Back closing a sheet first, the spinner's fixed shape, and a folder
// named like a special folder.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { SYSTEM_BAR_PROPS, applyInsets, readSystemBars, tellSystemBars } from "../lib/viewport";
import { Menu, Spinner, closeTopLayer, openLayers } from "../components/ui";
import { listLabel } from "../components/Sidebar";
import { setLang, t } from "../lib/i18n";
import type { Mailbox } from "../lib/types";
import { App } from "../App";
import { api } from "../lib/api";
import { click, flush, mounted } from "./harness";

async function render(node: React.ReactNode) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  mounted.push(root);
  await act(async () => root.render(node));
  return el;
}

const root = () => document.documentElement;
const bridge = () => window as unknown as { ArcaloSystemBars?: unknown };

afterEach(() => {
  for (const k of SYSTEM_BAR_PROPS) root().style.removeProperty(k);
  delete bridge().ArcaloSystemBars;
  delete root().dataset.layout;
});

describe("the system bars of Android", () => {
  it("sets the insets, the keyboard and the font size the shell reports, and nothing else", () => {
    expect(applyInsets(JSON.stringify({ "--sat": "48px", "--sar": "0px", "--sab": "24px", "--sal": "12.5px", "--ime": "0px", "--type-scale": "1.3" }))).toBe(true);
    expect(root().style.getPropertyValue("--sat")).toBe("48px");
    expect(root().style.getPropertyValue("--sab")).toBe("24px");
    expect(root().style.getPropertyValue("--sal")).toBe("12.5px");
    expect(root().style.getPropertyValue("--type-scale")).toBe("1.3");
    // Anything else, or a value that is not a plain length, is ignored.
    applyInsets(JSON.stringify({ "--sat": "calc(100vh)", "--sab": "url(x)", "--bg-app": "#000", "--type-scale": "2em", color: "red" }));
    expect(root().style.getPropertyValue("--sat")).toBe("48px");
    expect(root().style.getPropertyValue("--sab")).toBe("24px");
    expect(root().style.getPropertyValue("--bg-app")).toBe("");
    expect(root().style.getPropertyValue("--type-scale")).toBe("1.3");
    expect(applyInsets("not json")).toBe(false);
  });

  it("asks the shell at start and tells it the theme", () => {
    const setTheme = vi.fn();
    bridge().ArcaloSystemBars = { insets: () => JSON.stringify({ "--sat": "32px", "--sab": "48px" }), setTheme };
    readSystemBars();
    expect(root().style.getPropertyValue("--sat")).toBe("32px");
    expect(root().style.getPropertyValue("--sab")).toBe("48px");
    tellSystemBars(true, "#0e0f11");
    expect(setTheme).toHaveBeenCalledWith(true, "#0e0f11");
    tellSystemBars(false, undefined);
    expect(setTheme).toHaveBeenCalledTimes(1);
  });

  it("does nothing without the shell (desktop, iOS)", () => {
    readSystemBars();
    tellSystemBars(true, "#000000");
    expect(root().style.getPropertyValue("--sat")).toBe("");
  });
});

describe("the spinner", () => {
  it("is an SVG ring of its own size, which no span or flex rule of its container stretches", async () => {
    const el = await render(
      <div className="account-status">
        <Spinner />
        <span className="account-status-text">Synchronisiere…</span>
      </div>,
    );
    const s = el.querySelector(".spinner")!;
    expect(s.tagName.toLowerCase()).toBe("svg");
    expect(s.getAttribute("viewBox")).toBe("0 0 16 16");
    expect([s.getAttribute("width"), s.getAttribute("height")]).toEqual(["16", "16"]);
    expect(s.getAttribute("aria-hidden")).toBe("true");
    const labeled = await render(<Spinner label="Lädt" />);
    expect(labeled.querySelector(".spinner")!.getAttribute("role")).toBe("status");
  });
});

describe("folders named like special folders", () => {
  const box = (id: number, name: string, role: Mailbox["role"]): Mailbox => ({ id, account_id: 1, name, delimiter: "/", role, selectable: true, unread: 0, total: 0, sync_error: null }) as Mailbox;
  it("marks a label „Entwürfe“ next to Gmail's Drafts, and keeps brackets in names", () => {
    setLang("de");
    const all = [box(1, "INBOX", "inbox"), box(2, "[Gmail]/Entwürfe", "drafts"), box(3, "Entwürfe", null), box(4, "[Notion]", null), box(5, "Projekte", null)];
    expect(listLabel(all[1], all, t)).toBe("Entwürfe");
    expect(listLabel(all[2], all, t)).toBe("Entwürfe (Label)");
    // Elsewhere a folder of one's own.
    const imap = [box(1, "INBOX", "inbox"), box(2, "Drafts", "drafts"), box(3, "Entwürfe", null), box(4, "Projekte/Entwürfe", null)];
    expect(listLabel(imap[2], imap, t)).toBe("Entwürfe (Ordner)");
    expect(listLabel(imap[3], imap, t)).toBe("Projekte / Entwürfe (Ordner)");
    expect(listLabel(all[3], all, t)).toBe("[Notion]");
    expect(listLabel(all[4], all, t)).toBe("Projekte");
  });
});

describe("Gmail's folders in the folder list", () => {
  it("shows label names as they are, and tells the label „Entwürfe“ from the Drafts folder", async () => {
    const gmail: Mailbox[] = [
      ["INBOX", "inbox"],
      ["[Gmail]", null, false],
      ["[Gmail]/Entwürfe", "drafts"],
      ["[Gmail]/Gesendet", "sent"],
      ["[Gmail]/Alle Nachrichten", "all"],
      ["Entwürfe", null],
      ["[Notion]", null],
      ["[Notion]/Inbox", null],
      ["Account Confirmations", null],
    ].map(([name, role, selectable], i) => ({ id: 100 + i, account_id: 1, name, delimiter: "/", role, selectable: selectable !== false, unread: 0, total: 0 }) as Mailbox);
    const mailboxes = api.mailboxes;
    vi.spyOn(api, "mailboxes").mockImplementation(async (id: number) => (id === 1 ? gmail : mailboxes(id)));
    const appInfo = api.appInfo;
    vi.spyOn(api, "appInfo").mockImplementation(async () => ({ ...(await appInfo()), locale: "de-DE" }));
    const el = await render(<App />);
    for (let i = 0; i < 20 && !el.querySelector(".folder-name"); i++) await flush(30);
    const account = [...el.querySelectorAll(".sidebar .account")].find((x) => x.querySelector(".account-head"))!;
    const names = [...account.querySelectorAll(".folder-item .folder-name")].map((x) => x.textContent);
    expect(names).toEqual(["Posteingang", "Entwürfe", "Gesendet", "Alle Nachrichten", "Entwürfe (Label)", "[Notion]", "Inbox", "Account Confirmations"]);
    expect(account.querySelector(".folder-group")!.textContent).toBe("Labels");
    // The label's icon is a tag, the Drafts folder keeps its own.
    const icon = (name: string) => [...account.querySelectorAll(".folder")].find((b) => b.querySelector(".folder-name")!.textContent === name)!.querySelector("svg")!.getAttribute("class");
    expect(icon("Entwürfe (Label)")).toMatch(/tag/);
    expect(icon("Entwürfe")).toMatch(/file-text/);
  });
});

describe("a phone", () => {
  let width: PropertyDescriptor | undefined;
  beforeEach(() => {
    width = Object.getOwnPropertyDescriptor(window, "innerWidth");
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 390 });
  });
  afterEach(() => {
    if (width) Object.defineProperty(window, "innerWidth", width);
  });

  it("opens menus as sheets from the bottom, and Back closes the top one first", async () => {
    root().dataset.layout = "phone";
    const closed = vi.fn();
    await render(<Menu items={[{ label: "Eins", onSelect: () => {} }]} at={{ x: 300, y: 10 }} onClose={closed} label="Menü" />);
    const menu = document.querySelector(".menu")!;
    expect(menu.classList.contains("menu-sheet")).toBe(true);
    expect((menu as HTMLElement).style.top).toBe("");
    expect(document.querySelector(".sheet-scrim")).not.toBeNull();
    expect(openLayers()).toBe(1);
    expect(closeTopLayer()).toBe(true);
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("shows every tab with Focus, the sender's picture, and writes with an app bar", async () => {
    // The app on a touch screen (as on a phone), in German.
    const appInfo = api.appInfo;
    vi.spyOn(api, "appInfo").mockImplementation(async () => ({ ...(await appInfo()), locale: "de-DE", touch: true }));
    const el = await render(<App />);
    for (let i = 0; i < 20 && !el.querySelector(".messages .row[data-id]"); i++) await flush(30);
    expect(root().dataset.layout).toBe("phone");
    expect(el.querySelector(".app")!.getAttribute("data-layout")).toBe("phone");
    const tabs = [...el.querySelectorAll(".inbox-tabs .inbox-tab")].map((b) => b.getAttribute("data-tab"));
    expect(tabs).toEqual(["people", "newsletters", "notifications", "receipts", "other"]);
    expect(el.querySelector(".inbox-chips .inbox-focus-chip")).not.toBeNull();
    expect(el.querySelectorAll(".row[data-id] .row-avatar").length).toBeGreaterThan(0);
    expect(el.querySelector(".list-end")).not.toBeNull();

    await click(el.querySelector(".compose-fab")!);
    await flush();
    const compose = el.querySelector(".compose")!;
    expect(compose.querySelector(".compose-bar")).not.toBeNull();
    expect(compose.querySelector(".compose-foot")).toBeNull();
    expect(compose.querySelector(".compose-send")).not.toBeNull();
    // Cc and Bcc wait behind one button.
    const ccBcc = compose.querySelector(".compose-ccbcc")!;
    expect(ccBcc.textContent).toBe("Cc/Bcc");
    await click(ccBcc);
    expect(compose.querySelectorAll(".address-field, .field").length).toBeGreaterThan(3);
    // „More“: a sheet with what the footer held, Discard last.
    await click(compose.querySelector("[data-compose='more']")!);
    const sheet = document.querySelector(".menu-sheet")!;
    const labels = [...sheet.querySelectorAll(".menu-label")].map((x) => x.textContent);
    expect(labels[0]).toBe("Später senden");
    expect(labels[labels.length - 1]).toBe("Verwerfen");
  });
});
