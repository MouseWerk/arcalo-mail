// The list and the reading pane: search answers in the order asked, a message opened by its id
// is read like one opened in the list, Enter on a button is the button's, keys act only on a
// list that is shown, a slow archive does not move away from what was opened meanwhile, and a
// long conversation loads only the messages it shows.
import { describe, expect, it, vi } from "vitest";
import { act } from "react";
import { api } from "../lib/api";
import type { MessageSummary, Undo } from "../lib/types";
import { click, flush, key, mountApp, type } from "./harness";

const row = (id: number, subject = `Message ${id}`, extra: Partial<MessageSummary> = {}): MessageSummary => ({
  id,
  mailbox_id: 1,
  uid: id,
  thread_id: id,
  subject,
  from: { name: "X", email: "x@example.org" },
  date: 1_700_000_000 - id,
  snippet: "",
  seen: true,
  flagged: false,
  answered: false,
  has_attachments: false,
  thread_count: 1,
  account_id: 1,
  category: "people",
  ...extra,
});

describe("search", () => {
  it("ignores a slow answer for an older query", async () => {
    let releaseOld!: (v: MessageSummary[]) => void;
    vi.spyOn(api, "search").mockImplementation((q: string) => (q === "rech" ? new Promise((r) => (releaseOld = r)) : Promise.resolve([row(901, "Newer query hit")])));
    const { el } = await mountApp();
    const input = el.querySelector<HTMLInputElement>(".search-input")!;
    await type(input, "rech");
    await flush(200);
    await type(input, "rechnung");
    await flush(200);
    expect(el.querySelector('.row[data-id="901"]')).not.toBeNull();
    releaseOld([row(902, "Old query hit")]);
    await flush(50);
    expect(el.querySelector('.row[data-id="901"]')).not.toBeNull();
    expect(el.querySelector('.row[data-id="902"]')).toBeNull();
  });

  it("ignores an answer that comes after the field was cleared", async () => {
    let releaseOld!: (v: MessageSummary[]) => void;
    vi.spyOn(api, "search").mockImplementation(() => new Promise((r) => (releaseOld = r)));
    const { el } = await mountApp();
    const input = el.querySelector<HTMLInputElement>(".search-input")!;
    const title = () => el.querySelector(".list-title h1")?.textContent;
    const before = title();
    await type(input, "angebot");
    await flush(200);
    await type(input, "");
    await flush(50);
    releaseOld([row(903, "Late hit")]);
    await flush(50);
    expect(title()).toBe(before);
    expect(el.querySelector('.row[data-id="903"]')).toBeNull();
  });

  it("drops server hits for a query that changed meanwhile", async () => {
    let releaseServer!: (v: MessageSummary[]) => void;
    vi.spyOn(api, "search").mockImplementation(async (q: string) => (q === "alpha" ? [row(910, "alpha local")] : [row(920, "beta local")]));
    vi.spyOn(api, "serverSearch").mockImplementation(() => new Promise((r) => (releaseServer = r)));
    const { el } = await mountApp();
    const input = el.querySelector<HTMLInputElement>(".search-input")!;
    await type(input, "alpha");
    await flush(200);
    await act(async () => (el.querySelector(".list-foot button") as HTMLButtonElement).click());
    await flush(20);
    await type(input, "beta");
    await flush(200);
    releaseServer([row(911, "alpha server hit")]);
    await flush(50);
    expect(el.querySelector('.row[data-id="920"]')).not.toBeNull();
    expect(el.querySelector('.row[data-id="911"]')).toBeNull();
    expect(el.querySelector(".list-foot button")?.textContent).not.toContain("Auf dem Server gesucht");
  });
});

describe("a message opened by its id", () => {
  it("is marked read and shows its real state, as when opened in the list", async () => {
    const setFlags = vi.spyOn(api, "setFlags");
    let pending: { id: number; reply: boolean } | null = { id: 1, reply: false };
    vi.spyOn(api, "takePendingOpen").mockImplementation(async () => {
      const p = pending;
      pending = null;
      return p;
    });
    const { el } = await mountApp();
    await flush(100);
    expect(el.querySelector(".reading-pane .message-card")).not.toBeNull();
    expect(setFlags).toHaveBeenCalledWith([1], true, null);
  });

  it("opens its whole conversation", async () => {
    const thread = vi.spyOn(api, "thread");
    let pending: { id: number; reply: boolean } | null = { id: 3, reply: false };
    vi.spyOn(api, "takePendingOpen").mockImplementation(async () => {
      const p = pending;
      pending = null;
      return p;
    });
    const { el } = await mountApp();
    await flush(100);
    expect(thread).toHaveBeenCalledWith(3);
    expect(el.querySelectorAll(".reading-pane .message-card, .reading-pane .message-collapsed").length).toBe(2);
  });
});

describe("Enter", () => {
  it("on a toolbar button does not open the message again", async () => {
    const { el } = await mountApp();
    await click(el.querySelector('.row[data-id="1"]')!);
    await flush(100);
    const message = vi.spyOn(api, "message");
    const button = el.querySelector<HTMLButtonElement>('.reading-pane .toolbar [data-action="archive"]')!;
    button.focus();
    await key(button, "Enter");
    await flush(100);
    expect(message).not.toHaveBeenCalled();
  });

  it("on a sidebar folder does not open the first message", async () => {
    const { el } = await mountApp();
    const message = vi.spyOn(api, "message");
    const folder = el.querySelector<HTMLButtonElement>(".sidebar button.folder, nav button.folder")!;
    folder.focus();
    await key(folder, "Enter");
    await flush(100);
    expect(message).not.toHaveBeenCalled();
  });

  it("in the list still opens the message", async () => {
    const { el } = await mountApp();
    const message = vi.spyOn(api, "message");
    const list = el.querySelector<HTMLElement>(".messages")!;
    list.focus();
    await key(list, "Enter");
    await flush(100);
    expect(message).toHaveBeenCalledWith(1);
  });
});

describe("keys without a list on screen", () => {
  it("j in the attachment hub opens nothing", async () => {
    const { el } = await mountApp();
    await click(el.querySelector('.sidebar [data-view="files"]')!);
    await flush(100);
    expect(el.querySelector(".messages")).toBeNull();
    const message = vi.spyOn(api, "message");
    await key(document.body, "j");
    await flush(100);
    expect(message).not.toHaveBeenCalled();
  });

  it("e in the attachment hub archives nothing", async () => {
    const { el } = await mountApp();
    await click(el.querySelector('.sidebar [data-view="files"]')!);
    await flush(100);
    const archive = vi.spyOn(api, "archive");
    await key(document.body, "j");
    await flush(100);
    await key(document.body, "e");
    await flush(100);
    expect(archive).not.toHaveBeenCalled();
  });
});

describe("a slow archive", () => {
  it("leaves the message opened meanwhile open", async () => {
    const rows = [101, 102, 103, 104, 105].map((id) => row(id));
    vi.spyOn(api, "inbox").mockImplementation(async (_q, offset, limit) => ({ total: rows.length, offset, items: rows.slice(offset, offset + limit) }));
    let finish!: (u: Undo) => void;
    vi.spyOn(api, "archive").mockImplementation(() => new Promise((r) => (finish = r)));
    const { el } = await mountApp();
    await flush(50);
    await click(el.querySelector('.row[data-id="101"]')!);
    await flush(50);
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "e");
    await click(el.querySelector('.row[data-id="104"]')!);
    await flush(50);
    await act(async () => finish({ outbox: [], hidden: [101], accounts: [] }));
    await flush(100);
    expect(el.querySelector('.row[aria-current="true"]')?.getAttribute("data-id")).toBe("104");
  });

  it("still moves on to the next message when the archived one is open", async () => {
    const rows = [101, 102, 103].map((id) => row(id));
    vi.spyOn(api, "inbox").mockImplementation(async (_q, offset, limit) => ({ total: rows.length, offset, items: rows.slice(offset, offset + limit) }));
    vi.spyOn(api, "archive").mockImplementation(async () => ({ outbox: [], hidden: [101], accounts: [] }));
    const { el } = await mountApp();
    await flush(50);
    await click(el.querySelector('.row[data-id="101"]')!);
    await flush(50);
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "e");
    await flush(100);
    expect(el.querySelector('.row[aria-current="true"]')?.getAttribute("data-id")).toBe("102");
  });
});

describe("a long conversation", () => {
  it("loads only the newest unread messages and the others when expanded", async () => {
    const many = Array.from({ length: 40 }, (_, i) => row(500 + i, "Liste", { thread_id: 500, thread_count: 40, seen: false, date: 1_700_000_000 + i }));
    vi.spyOn(api, "inbox").mockImplementation(async (_q, offset) => ({ total: 1, offset, items: [many[39]] }));
    vi.spyOn(api, "thread").mockResolvedValue(many);
    const message = vi.spyOn(api, "message");
    const { el } = await mountApp();
    await flush(50);
    await click(el.querySelector('.row[data-id="539"]')!);
    await flush(100);
    expect(message.mock.calls.length).toBeLessThanOrEqual(5);
    expect(message).toHaveBeenCalledWith(539);
    expect(message).not.toHaveBeenCalledWith(500);
  });
});
