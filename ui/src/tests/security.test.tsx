// Security in the UI: the gate shows the app, the lock screen or the recovery screen; the lock
// screen unlocks with the PIN and counts down after wrong ones; Settings → Sicherheit asks for a
// PIN before the lock goes on and shows the recovery key before encrypting; the phishing warning
// names its reasons and silences a sender.
import { describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { api } from "../lib/api";
import { setLang } from "../lib/i18n";
import { DEFAULT_LOCK, security, type CipherStatus, type LockStatus } from "../lib/security";
import { SecurityGate, SecuritySection } from "../components/Security";
import { PhishingNotice } from "../components/Phishing";
import { demo } from "../lib/demo";
import { click, flush, mounted, type } from "./harness";

const open: LockStatus = { config: DEFAULT_LOCK, has_pin: false, locked: false, wait_secs: 0, failures: 0, biometric: null, encrypted: false, gate: null, password: false, phone: false };
const plain: CipherStatus = { state: "plain", key_stored: false, store_file: false, password: false, pending: null, old_left: false, notice: null };

async function render(node: React.ReactNode): Promise<HTMLElement> {
  setLang("de");
  vi.spyOn(api, "appInfo").mockImplementation(async () => ({ ...(await demo.appInfo()), locale: "de-DE" }));
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  mounted.push(root);
  await act(async () => root.render(node));
  await flush(30);
  return el;
}

describe("the security gate", () => {
  it("shows the app when nothing is locked", async () => {
    vi.spyOn(security, "state").mockResolvedValue(open);
    const el = await render(
      <SecurityGate>
        <p data-app>Posteingang</p>
      </SecurityGate>,
    );
    expect(el.querySelector("[data-app]")).not.toBeNull();
    expect(el.querySelector("[data-lock-screen]")).toBeNull();
  });

  it("hides the app behind the lock screen and unlocks with the PIN", async () => {
    let locked = true;
    vi.spyOn(security, "state").mockImplementation(async () => ({ ...open, has_pin: true, locked, config: { ...DEFAULT_LOCK, mode: "start" } }));
    const unlock = vi.spyOn(security, "unlock").mockImplementation(async (pin) => {
      if (pin !== "4711") return { ok: false, wait_secs: 5, failures: 3 };
      locked = false;
      return { ok: true, wait_secs: 0, failures: 0 };
    });
    const el = await render(
      <SecurityGate>
        <p data-app>Geheime Betreffzeile</p>
      </SecurityGate>,
    );
    expect(el.querySelector("[data-lock-screen]")).not.toBeNull();
    expect(el.textContent).not.toContain("Geheime Betreffzeile");
    const input = el.querySelector<HTMLInputElement>(".lock-pin")!;
    await type(input, "0000");
    await act(async () => el.querySelector("form")!.requestSubmit());
    await flush(30);
    expect(el.querySelector(".lock-msg")!.textContent).toContain("0:05");
    expect(input.disabled).toBe(true);
    // The wait is over (as the countdown would end it): the right PIN opens the app.
    vi.spyOn(security, "state").mockImplementation(async () => ({ ...open, has_pin: true, locked, config: { ...DEFAULT_LOCK, mode: "start" } }));
    unlock.mockClear();
    locked = true;
    await act(async () => {
      await new Promise((r) => setTimeout(r, 5200));
    });
    await type(el.querySelector<HTMLInputElement>(".lock-pin")!, "4711");
    await act(async () => el.querySelector("form")!.requestSubmit());
    await flush(50);
    expect(unlock).toHaveBeenCalledWith("4711");
    expect(el.querySelector("[data-app]")).not.toBeNull();
  }, 15_000);

  it("offers the recovery key when the key of an encrypted database is missing", async () => {
    vi.spyOn(security, "state").mockResolvedValue({ ...open, gate: "missing_key", encrypted: true });
    const gateUnlock = vi.spyOn(security, "gateUnlock").mockResolvedValue(undefined);
    const el = await render(
      <SecurityGate>
        <p data-app>App</p>
      </SecurityGate>,
    );
    expect(el.querySelector("[data-key-gate]")).not.toBeNull();
    expect(el.querySelector("[data-app]")).toBeNull();
    const open_ = el.querySelector<HTMLButtonElement>("button[type=submit]")!;
    expect(open_.disabled).toBe(true);
    await type(el.querySelector<HTMLInputElement>(".lock-code")!, "a".repeat(56));
    expect(el.querySelector<HTMLInputElement>(".lock-code")!.value).toMatch(/^AAAA-AAAA-/);
    await act(async () => el.querySelector("form")!.requestSubmit());
    await flush(20);
    expect(gateUnlock).toHaveBeenCalledWith("recovery", expect.stringMatching(/^AAAA-/), true);
  });
});

describe("Settings → Sicherheit", () => {
  it("asks for a PIN before the lock goes on", async () => {
    vi.spyOn(security, "lockStatus").mockResolvedValue(open);
    vi.spyOn(security, "cipherStatus").mockResolvedValue(plain);
    const configure = vi.spyOn(security, "configureLock").mockImplementation(async (config) => ({ ...open, config, has_pin: true }));
    const el = await render(<SecuritySection settings={await demo.settings()} onChange={() => {}} />);
    const start = Array.from(el.querySelectorAll<HTMLButtonElement>(".segmented button, [role=radio]")).find((b) => b.textContent === "Beim Start")!;
    await click(start);
    await flush(20);
    const dialog = document.querySelector<HTMLElement>("[role=dialog]")!;
    expect(dialog.textContent).toContain("PIN festlegen");
    await type(dialog.querySelector<HTMLInputElement>("[data-pin-new]")!, "4711");
    await type(dialog.querySelector<HTMLInputElement>("[data-pin-repeat]")!, "4712");
    await click(dialog.querySelector("[data-pin-save]")!);
    await flush(20);
    expect(dialog.textContent).toContain("verschieden");
    expect(configure).not.toHaveBeenCalled();
    await type(dialog.querySelector<HTMLInputElement>("[data-pin-repeat]")!, "4711");
    await click(dialog.querySelector("[data-pin-save]")!);
    await flush(30);
    expect(configure).toHaveBeenCalledWith({ ...DEFAULT_LOCK, mode: "start" }, "4711");
    expect(el.querySelector("[data-lock-now]")).not.toBeNull();
  });

  it("shows the recovery key and encrypts only once it is kept", async () => {
    vi.spyOn(security, "lockStatus").mockResolvedValue(open);
    vi.spyOn(security, "cipherStatus").mockResolvedValue(plain);
    vi.spyOn(security, "recoveryKey").mockResolvedValue("ABCD-EFGH-IJKL");
    const sw = vi.spyOn(security, "switchCipher").mockResolvedValue({ ...plain, state: "encrypted", key_stored: true });
    const el = await render(<SecuritySection settings={await demo.settings()} onChange={() => {}} />);
    expect(el.querySelector("[data-cipher-state]")!.getAttribute("data-cipher-state")).toBe("plain");
    await click(el.querySelector("[data-encrypt]")!);
    await flush(20);
    expect(document.querySelector("[data-recovery-code]")!.textContent).toBe("ABCD-EFGH-IJKL");
    const confirm = document.querySelector<HTMLButtonElement>("[data-encrypt-confirm]")!;
    expect(confirm.disabled).toBe(true);
    await click(document.querySelector("[role=dialog] [role=switch]")!);
    await flush(10);
    expect(confirm.disabled).toBe(false);
    await click(confirm);
    await flush(30);
    expect(sw).toHaveBeenCalledWith(true);
  });
});

describe("the phishing warning", () => {
  it("names its reasons and silences the sender", async () => {
    const ok = vi.spyOn(api, "phishingOk").mockResolvedValue(undefined);
    const changed = vi.fn();
    const el = await render(
      <PhishingNotice
        warning={{
          sender: "service@paypa1.com",
          reasons: [
            { reason: "lookalike", domain: "paypa1.com", like: "paypal.com" },
            { reason: "auth_failed", checks: ["spf", "dmarc"] },
          ],
        }}
        onChanged={changed}
      />,
    );
    const items = Array.from(el.querySelectorAll("[data-reason]")).map((li) => li.textContent);
    expect(items[0]).toContain("paypa1.com");
    expect(items[0]).toContain("paypal.com");
    expect(items[1]).toContain("DMARC und SPF");
    await click(el.querySelector("[data-phishing-ok]")!);
    await flush(20);
    expect(ok).toHaveBeenCalledWith("service@paypa1.com", true);
    expect(changed).toHaveBeenCalled();
  });
});
