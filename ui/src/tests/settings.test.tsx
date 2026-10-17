// Settings: an Undo takes back only its own change, fields saved on leaving them are saved when
// the settings close, and a Microsoft sign-in that was left adds no account.
import { describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { api } from "../lib/api";
import { Setup } from "../components/Setup";
import type { MicrosoftSignIn } from "../lib/types";
import { click, flush, key, mounted, mountApp, type } from "./harness";

describe("Undo of a setting", () => {
  it("takes back only that change", async () => {
    const { el } = await mountApp();
    await key(document.body, ",", { ctrlKey: true });
    await flush(50);
    const nav = Array.from(el.querySelectorAll<HTMLButtonElement>(".settings-nav-item"));
    await click(nav[1]);
    await flush(20);
    const switches = () => Array.from(el.querySelectorAll<HTMLButtonElement>(".settings [role=switch]"));
    const threadsBefore = switches()[0].getAttribute("aria-checked");
    const snippetsBefore = switches()[2].getAttribute("aria-checked");
    await click(switches()[0]);
    await flush(50);
    await click(switches()[2]);
    await flush(50);
    const undoFirst = el.querySelectorAll<HTMLButtonElement>(".toast-action")[0];
    await act(async () => undoFirst.click());
    await flush(80);
    expect(switches()[0].getAttribute("aria-checked")).toBe(threadsBefore);
    expect(switches()[2].getAttribute("aria-checked")).not.toBe(snippetsBefore);
    expect((await api.settings()).show_snippets).toBe(snippetsBefore !== "true");
  });

  it("is not undone by the reload of an earlier save", async () => {
    const { el } = await mountApp();
    let releaseFirst!: () => void;
    const real = api.saveSettings;
    let n = 0;
    vi.spyOn(api, "saveSettings").mockImplementation(async (s) => {
      if (n++ === 0) await new Promise<void>((r) => (releaseFirst = r));
      return real(s);
    });
    await key(document.body, ",", { ctrlKey: true });
    await flush(50);
    await click(Array.from(el.querySelectorAll<HTMLButtonElement>(".settings-nav-item"))[1]);
    await flush(20);
    const switches = () => Array.from(el.querySelectorAll<HTMLButtonElement>(".settings [role=switch]"));
    const snippetsBefore = switches()[2].getAttribute("aria-checked");
    await click(switches()[0]);
    await flush(20);
    await click(switches()[2]);
    await flush(20);
    releaseFirst();
    await flush(80);
    expect(switches()[2].getAttribute("aria-checked")).not.toBe(snippetsBefore);
  });
});

describe("fields saved on leaving them", () => {
  it("are saved when the settings close with Esc", async () => {
    const update = vi.spyOn(api, "updateAccount");
    const { el } = await mountApp();
    await key(document.body, ",", { ctrlKey: true });
    await flush(50);
    const sig = el.querySelector<HTMLTextAreaElement>(".settings textarea")!;
    sig.focus();
    await type(sig, "Viele Grüße\nAnna");
    await key(sig, "Escape");
    await flush(50);
    expect(el.querySelector(".settings")).toBeNull();
    expect(update).toHaveBeenCalledTimes(1);
    expect(update.mock.calls[0][3]).toBe("Viele Grüße\nAnna");
  });
});

describe("Microsoft sign-in", () => {
  it("adds no account after Back", async () => {
    vi.spyOn(api, "appInfo").mockResolvedValue({ ...(await api.appInfo()), microsoft_sign_in: true });
    let finish!: (r: MicrosoftSignIn) => void;
    vi.spyOn(api, "microsoftSignIn").mockImplementation(() => new Promise((r) => (finish = r)));
    const add = vi.spyOn(api, "addMicrosoftAccount");
    const el = document.createElement("div");
    document.body.appendChild(el);
    const root = createRoot(el);
    mounted.push(root);
    await act(async () => root.render(<Setup welcome={false} onDone={() => {}} />));
    await flush(20);
    await type(el.querySelector<HTMLInputElement>('input[type="email"]')!, "anna@outlook.com");
    await act(async () => el.querySelector<HTMLFormElement>("form")!.requestSubmit());
    await flush(20);
    expect(el.querySelector(".setup-microsoft")).not.toBeNull();
    await click(el.querySelector(".setup-microsoft .setup-actions .btn")!);
    await flush(20);
    await act(async () => finish({ session: "s1", email: "anna@outlook.com", check: { imap: null, smtp: null, error: null, folders: 3 }, problem: null, error: null }));
    await flush(50);
    expect(add).not.toHaveBeenCalled();
    expect(el.querySelector(".setup-microsoft")).toBeNull();
  });
});
