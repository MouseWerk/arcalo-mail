// The opening animation: it never holds the app back, it takes the last theme's colors and only
// well-formed ones, it stays away when switched off, and reduced motion keeps it still.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { rememberSplash, startSplash } from "./splash";

const splash = () => {
  const el = document.createElement("div");
  el.id = "splash";
  document.body.appendChild(el);
  return el;
};

describe("the opening animation", () => {
  beforeEach(() => {
    localStorage.clear();
    // happy-dom reports itself as automated, which skips the splash.
    localStorage.setItem("arcalo-mail.splash-test", "1");
    document.body.className = "";
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    document.getElementById("splash")?.remove();
  });

  it("fades out as soon as the app is ready, however far it got", async () => {
    const el = splash();
    startSplash();
    expect(el.classList.contains("out")).toBe(false);
    document.body.classList.add("ready");
    await vi.advanceTimersByTimeAsync(0);
    expect(el.classList.contains("out")).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(document.getElementById("splash")).toBeNull();
  });

  it("goes after a few seconds whatever happens", async () => {
    splash();
    startSplash();
    await vi.advanceTimersByTimeAsync(8200);
    expect(document.getElementById("splash")).toBeNull();
  });

  it("is not shown when switched off", () => {
    rememberSplash({ off: true });
    splash();
    startSplash();
    expect(document.getElementById("splash")).toBeNull();
  });

  it("takes the last theme's colors and ignores anything that is not a color", () => {
    rememberSplash({ off: false, bg: "#0e0f11", ink: "#ececef", seal: "#818cf8" });
    const el = splash();
    startSplash();
    expect(el.style.getPropertyValue("--splash-bg")).toBe("#0e0f11");
    expect(el.style.getPropertyValue("--splash-seal")).toBe("#818cf8");
    el.remove();
    localStorage.setItem("arcalo-mail.splash", JSON.stringify({ bg: "red; background: url(x)", ink: "#12345" }));
    const el2 = splash();
    startSplash();
    expect(el2.style.getPropertyValue("--splash-bg")).toBe("");
    expect(el2.style.getPropertyValue("--splash-ink")).toBe("");
  });

  it("is short, lets clicks through and stands still with reduced motion", () => {
    const css = readFileSync(resolve(__dirname, "../../public/splash.css"), "utf8");
    expect(css).toMatch(/#splash \{[^}]*pointer-events: none/);
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\)/);
    // Every delay plus duration ends within 700 ms.
    const ends = [...css.matchAll(/animation:[^;]*?(\d+)ms[^;]*?(?:(\d+)ms)?[^;]*?forwards/g)].map((m) => Number(m[1]) + Number(m[2] ?? 0));
    expect(ends.length).toBeGreaterThan(2);
    expect(Math.max(...ends)).toBeLessThanOrEqual(700);
    const html = readFileSync(resolve(__dirname, "../../index.html"), "utf8");
    expect(html).toContain('<div id="splash" aria-hidden="true">');
  });
});
