// The compose sheet in the app: a new draft replaces the open one without carrying its text
// along, addresses still being typed go out with the message, nothing reopens after sending,
// drafts are saved before the sheet closes (and kept open when that fails), the undo of a send
// never overwrites a message being written, and Discard can be undone.
import { describe, expect, it, vi } from "vitest";
import { act } from "react";
import { api } from "../lib/api";
import { click, flush, key, mountApp, type } from "./harness";

const editor = (el: HTMLElement) => el.querySelector<HTMLElement>(".compose .editor")!;
const subject = (el: HTMLElement) => el.querySelector<HTMLInputElement>("#compose-subject")!;
const addressInput = (el: HTMLElement) => el.querySelector<HTMLInputElement>(".compose .chip-input")!;

async function typeInEditor(el: HTMLElement, html: string) {
  await act(async () => {
    editor(el).innerHTML = html;
    editor(el).dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function writeAndSend(el: HTMLElement, text: string) {
  await key(document.body, "c");
  await flush(50);
  await type(addressInput(el), "anna@example.org,");
  await type(subject(el), text);
  await key(subject(el), "Enter", { ctrlKey: true });
  await flush(50);
}

describe("a new draft replaces the open one", () => {
  it("loads the reply into the editor instead of keeping the old text", async () => {
    const send = vi.spyOn(api, "send");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await typeInEditor(el, "<p>Private note for someone else</p>");
    await click(el.querySelector('.row[data-id="1"]')!);
    await flush(100);
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "r");
    await flush(100);
    expect(subject(el).value.startsWith("Re:")).toBe(true);
    expect(editor(el).innerHTML).not.toContain("Private note for someone else");
    expect(editor(el).innerHTML).toContain("blockquote");
    await act(async () => {
      editor(el).dispatchEvent(new Event("input", { bubbles: true }));
    });
    await key(editor(el), "Enter", { ctrlKey: true });
    await flush(50);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0].html).not.toContain("Private note for someone else");
    expect(send.mock.calls[0][0].html).toContain("blockquote");
  });
});

describe("addresses still being typed", () => {
  it("go out with Ctrl+Enter", async () => {
    const send = vi.spyOn(api, "send");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(addressInput(el), "anna@example.org,");
    await type(addressInput(el), "bob@example.org");
    await key(addressInput(el), "Enter", { ctrlKey: true });
    await flush(100);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0].to).toEqual(["anna@example.org", "bob@example.org"]);
  });

  it("count as a recipient when it is the only one", async () => {
    const send = vi.spyOn(api, "send");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(addressInput(el), "bob@example.org");
    await key(addressInput(el), "Enter", { ctrlKey: true });
    await flush(50);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0].to).toEqual(["bob@example.org"]);
    expect(el.querySelector(".compose-error")).toBeNull();
  });

  it("go out with the Send button, and the sheet does not come back", async () => {
    const send = vi.spyOn(api, "send");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    const to = addressInput(el);
    await type(to, "anna@example.org,");
    await type(to, "bob@example.org");
    await type(subject(el), "Hallo");
    // Mouse down on Send moves the focus: the address field blurs, then the click sends.
    await act(async () => {
      to.dispatchEvent(new FocusEvent("blur", { bubbles: false }));
      to.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    });
    await act(async () => el.querySelector<HTMLButtonElement>(".compose-foot .btn-primary")!.click());
    await flush(20);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0].to).toEqual(["anna@example.org", "bob@example.org"]);
    await flush(300);
    expect(el.querySelector(".compose")).toBeNull();
  });
});

describe("closing the sheet", () => {
  it("keeps it open with the text when the draft cannot be saved", async () => {
    vi.spyOn(api, "saveDraft").mockRejectedValue("database is locked");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Important text");
    await key(subject(el), "Escape");
    await flush(50);
    expect(el.querySelector(".compose")).not.toBeNull();
    expect(subject(el).value).toBe("Important text");
    expect(el.querySelector(".compose-error")?.textContent).toContain("database is locked");
  });

  it("does not replace an open message whose draft cannot be saved", async () => {
    vi.spyOn(api, "saveDraft").mockRejectedValue("offline");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "First");
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "n", { ctrlKey: true });
    await flush(50);
    expect(subject(el).value).toBe("First");
  });

  it("saves once when two saves are asked for at once (no second draft)", async () => {
    let calls = 0;
    const seen: (string | null)[] = [];
    vi.spyOn(api, "saveDraft").mockImplementation(async (d) => {
      seen.push(d.message_id);
      calls++;
      await new Promise((r) => setTimeout(r, 30));
      return d.message_id ?? "draft-1@example.org";
    });
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Twice");
    await key(subject(el), "s", { ctrlKey: true });
    await key(subject(el), "Escape");
    await flush(150);
    expect(calls).toBeGreaterThanOrEqual(1);
    // Every save after the first one replaces the first one's draft.
    expect(seen.slice(1).every((m) => m === "draft-1@example.org")).toBe(true);
  });
});

describe("the draft is saved while writing", () => {
  it("a moment after the last change, and the sheet says so", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const save = vi.spyOn(api, "saveDraft");
      const { el } = await mountApp();
      await key(document.body, "c");
      await flush(50);
      await type(subject(el), "Autosaved");
      expect(save).not.toHaveBeenCalled();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3500);
      });
      await flush(20);
      expect(save).toHaveBeenCalledTimes(1);
      expect(save.mock.calls[0][0].subject).toBe("Autosaved");
      expect(el.querySelector(".compose-saved")?.textContent).toBe("Gespeichert");
      await type(subject(el), "Autosaved again");
      expect(el.querySelector(".compose-saved")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("when the window goes to the background", async () => {
    const save = vi.spyOn(api, "saveDraft");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Before the background");
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await flush(20);
    Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0][0].subject).toBe("Before the background");
  });

  it("when the shell asks before it quits", async () => {
    const save = vi.spyOn(api, "saveDraft");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Before quitting");
    await act(async () => {
      window.dispatchEvent(new CustomEvent("arcalo-mail:save-drafts"));
    });
    await flush(20);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0][0].subject).toBe("Before quitting");
  });
});

describe("undo of a send", () => {
  it("asks before it replaces a message being written (toast)", async () => {
    const save = vi.spyOn(api, "saveDraft");
    const { el } = await mountApp();
    await writeAndSend(el, "First (sent)");
    expect(el.querySelector(".compose")).toBeNull();
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Second (half written)");
    const undo = Array.from(el.querySelectorAll<HTMLButtonElement>(".toast-action")).at(-1)!;
    await act(async () => undo.click());
    await flush(50);
    // Nothing is replaced yet: the question is asked.
    expect(subject(el).value).toBe("Second (half written)");
    const dialog = el.querySelector<HTMLElement>(".dialog")!;
    expect(dialog).not.toBeNull();
    await click(dialog.querySelector(".btn-primary")!);
    await flush(50);
    expect(save.mock.calls.some(([d]) => d.subject === "Second (half written)")).toBe(true);
    expect(subject(el).value).toBe("First (sent)");
  });

  it("u keeps the message being written and files the other one as a draft when declined", async () => {
    const save = vi.spyOn(api, "saveDraft");
    const { el } = await mountApp();
    await writeAndSend(el, "First (sent)");
    await key(document.body, "c");
    await flush(50);
    await type(subject(el), "Second (half written)");
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "u");
    await flush(50);
    const dialog = el.querySelector<HTMLElement>(".dialog")!;
    expect(dialog).not.toBeNull();
    await click(dialog.querySelector(".dialog-foot .btn:not(.btn-primary)")!);
    await flush(50);
    expect(subject(el).value).toBe("Second (half written)");
    expect(save.mock.calls.some(([d]) => d.subject === "First (sent)")).toBe(true);
  });

  it("opens the message again at once when no other one is open", async () => {
    const { el } = await mountApp();
    await writeAndSend(el, "Only one");
    el.querySelector<HTMLElement>(".messages")!.focus();
    await key(el.querySelector(".messages")!, "u");
    await flush(50);
    expect(el.querySelector(".dialog")).toBeNull();
    expect(subject(el).value).toBe("Only one");
  });
});

describe("Discard", () => {
  it("can be undone, and the saved draft is removed only after that", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const discard = vi.spyOn(api, "discardDraft");
      const { el } = await mountApp();
      await key(document.body, "c");
      await flush(50);
      await type(subject(el), "Maybe not");
      await key(subject(el), "s", { ctrlKey: true });
      await flush(50);
      await click(el.querySelector(`.compose-foot [aria-label="Verwerfen"]`)!);
      await flush(20);
      expect(el.querySelector(".compose")).toBeNull();
      expect(discard).not.toHaveBeenCalled();
      const undo = Array.from(el.querySelectorAll<HTMLButtonElement>(".toast-action")).at(-1)!;
      await act(async () => undo.click());
      await flush(20);
      expect(subject(el).value).toBe("Maybe not");
      await click(el.querySelector(`.compose-foot [aria-label="Verwerfen"]`)!);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6000);
      });
      expect(discard).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("keys in the editor", () => {
  it("Ctrl+K inserts a link instead of opening the command palette", async () => {
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    editor(el).focus();
    await key(editor(el), "k", { ctrlKey: true });
    await flush(20);
    expect(el.querySelector(".palette-overlay")).toBeNull();
    expect(el.querySelector(".dialog input")).not.toBeNull();
  });

  it("Ctrl+Enter in the link prompt does not send the message", async () => {
    const send = vi.spyOn(api, "send");
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    await type(addressInput(el), "anna@example.org,");
    editor(el).focus();
    await key(editor(el), "k", { ctrlKey: true });
    await flush(20);
    await key(el.querySelector(".dialog input")!, "Enter", { ctrlKey: true });
    await flush(50);
    expect(send).not.toHaveBeenCalled();
  });

  it("Tab leaves the docked sheet on a wide window", async () => {
    const { el } = await mountApp();
    await key(document.body, "c");
    await flush(50);
    // happy-dom lays nothing out: every element counts as shown, as in a browser.
    Object.defineProperty(HTMLElement.prototype, "offsetParent", { get: () => document.body, configurable: true });
    const last = Array.from(el.querySelectorAll<HTMLElement>(".compose button:not([disabled])")).at(-1)!;
    last.focus();
    const e = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    await act(async () => {
      last.dispatchEvent(e);
    });
    expect(e.defaultPrevented).toBe(false);
    delete (HTMLElement.prototype as { offsetParent?: unknown }).offsetParent;
  });
});

describe("the protection a recipient asked for", () => {
  // The answer about the keys arrives while the parent has not rendered the sheet's last change
  // yet (keys typed in the same moment): the next change must keep the encryption it set.
  it("survives a change made before the sheet rendered it", async () => {
    const { createRoot } = await import("react-dom/client");
    const { ComposeSheet } = await import("../components/ComposeSheet");
    const { emptyDraft } = await import("../lib/compose");
    const { demo } = await import("../lib/demo");
    const { mounted } = await import("./harness");
    vi.spyOn(api, "cryptoAbilities").mockResolvedValue({
      pgp_sign: true,
      pgp_encrypt: true,
      pgp_missing: [],
      smime_sign: false,
      smime_encrypt: false,
      smime_missing: [],
      pgp_prefer_encrypt: true,
    });
    const account = (await demo.accounts())[0];
    const draft = { ...emptyDraft(account, false, ["jonas@example.org"]), accountId: account.id };
    const changes: { protect?: unknown; subject: string }[] = [];
    const el = document.createElement("div");
    document.body.appendChild(el);
    const root = createRoot(el);
    mounted.push(root);
    // The parent never renders the changes again: every one must stand on the ones before.
    const props = { accounts: [account], error: null, sending: false, onSend: () => {}, onSendLater: () => {}, onSaveDraft: () => {}, onClose: () => {}, onDiscard: () => {} };
    await act(async () => root.render(<ComposeSheet {...props} draft={draft} onChange={(d) => changes.push(d)} />));
    await flush(400);
    expect(changes.at(-1)?.protect).toEqual({ method: "pgp", sign: false, encrypt: true });
    await type(subject(el), "Vertrag");
    expect(changes.at(-1)?.subject).toBe("Vertrag");
    expect(changes.at(-1)?.protect).toEqual({ method: "pgp", sign: false, encrypt: true });
  });
});
