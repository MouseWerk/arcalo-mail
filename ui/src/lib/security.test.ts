// The pieces of logic the lock screen, the recovery screen and Settings → Sicherheit share.
import { describe, expect, it } from "vitest";
import { formatRecoveryInput, formatWait, isLockedError, passwordProblem, pinProblem, recoveryComplete, watchActivity } from "./security";
import { reasonText } from "../components/Phishing";

describe("security helpers", () => {
  it("formats a recovery key as typed", () => {
    expect(formatRecoveryInput("abcd efgh-0I18")).toBe("ABCD-EFGH-OIIB");
    expect(formatRecoveryInput("xyz9!")).toBe("XYZ");
    const full = "A".repeat(60);
    expect(formatRecoveryInput(full).replace(/-/g, "")).toHaveLength(56);
    expect(recoveryComplete(formatRecoveryInput(full))).toBe(true);
    expect(recoveryComplete("ABCD-EFGH")).toBe(false);
  });

  it("checks new PINs and passwords", () => {
    expect(pinProblem("123", "123")).toBe("short");
    expect(pinProblem("1234", "1235")).toBe("mismatch");
    expect(pinProblem("x".repeat(65), "x".repeat(65))).toBe("long");
    expect(pinProblem("4711", "4711")).toBeNull();
    expect(passwordProblem("kurz", "kurz")).toBe("short");
    expect(passwordProblem("lange Passphrase", "lange passphrase")).toBe("mismatch");
    expect(passwordProblem("lange Passphrase", "lange Passphrase")).toBeNull();
  });

  it("shows the wait after wrong PINs and knows a refusal while locked", () => {
    expect(formatWait(5)).toBe("0:05");
    expect(formatWait(125)).toBe("2:05");
    expect(formatWait(-3)).toBe("0:00");
    expect(isLockedError("app-locked")).toBe(true);
    expect(isLockedError(new Error("other"))).toBe(false);
  });

  it("reports input at most every few seconds", () => {
    const target = new EventTarget();
    let n = 0;
    const stop = watchActivity(() => n++, 60_000, target as unknown as Window);
    target.dispatchEvent(new Event("keydown"));
    target.dispatchEvent(new Event("pointermove"));
    expect(n).toBe(1);
    stop();
    expect(n).toBe(1);
  });

  it("words every reason of a phishing warning", () => {
    expect(reasonText({ reason: "lookalike", domain: "paypa1.com", like: "paypal.com" })).toEqual(["phish.lookalike", { domain: "paypa1.com", like: "paypal.com" }]);
    expect(reasonText({ reason: "link", shown: "a.de", target: "b.ru", more: 0 })[0]).toBe("phish.link");
    expect(reasonText({ reason: "link", shown: "a.de", target: "b.ru", more: 2 })).toEqual(["phish.linkMore", { shown: "a.de", target: "b.ru", n: 2 }]);
    expect(reasonText({ reason: "auth_failed", checks: ["spf"] })[0]).toBe("phish.authSpf");
    expect(reasonText({ reason: "auth_failed", checks: ["dmarc"] })[0]).toBe("phish.authDmarc");
    expect(reasonText({ reason: "auth_failed", checks: ["spf", "dmarc"] })[0]).toBe("phish.authBoth");
    expect(reasonText({ reason: "reply_to", reply_to: "x@gmail.com", from: "firma.de" })).toEqual(["phish.replyTo", { reply: "x@gmail.com", domain: "firma.de" }]);
  });
});
