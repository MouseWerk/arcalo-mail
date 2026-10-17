// Renders the real <App /> against the demo backend (lib/demo.ts) in happy-dom, with helpers
// for keys, clicks and typing into React-controlled fields.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { App } from "../App";
import { api } from "../lib/api";
import { afterEach, vi } from "vitest";

afterEach(async () => {
  await act(async () => {
    for (const r of mounted.splice(0)) r.unmount();
  });
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
// happy-dom has no document.images (the frame's onLoad reads it).
if (!Object.getOwnPropertyDescriptor(Document.prototype, "images"))
  Object.defineProperty(Document.prototype, "images", { get(this: Document) { return this.querySelectorAll("img"); } });

export const wait = (ms = 0) => new Promise((r) => setTimeout(r, ms));
export async function flush(ms = 30) {
  await act(async () => {
    await wait(ms);
  });
}

export const mounted: Root[] = [];

/** The app in German (the tests read its German texts), whatever the machine's language is. */
export async function mountApp(): Promise<{ root: Root; el: HTMLElement }> {
  const appInfo = api.appInfo;
  vi.spyOn(api, "appInfo").mockImplementation(async () => ({ ...(await appInfo()), locale: "de-DE" }));
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  mounted.push(root);
  await act(async () => root.render(<App />));
  for (let i = 0; i < 20 && !el.querySelector(".messages .row[data-id]"); i++) await flush(30);
  return { root, el };
}

export async function key(target: EventTarget, k: string, init: KeyboardEventInit = {}) {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
  });
}

export async function click(el: Element) {
  await act(async () => {
    (el as HTMLElement).click();
  });
}

/** Types into a React-controlled input. */
export async function type(input: HTMLInputElement | HTMLTextAreaElement, value: string) {
  await act(async () => {
    const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
