// The page and the Content Security Policy: no inline <style> or <script> in index.html (the CSP
// forbids inline scripts), and a CSP without eval or inline scripts, as in Arcalo.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(__dirname, "../../..");

describe("index.html and CSP", () => {
  it("has no inline style or script", () => {
    const html = fs.readFileSync(path.join(ROOT, "ui/index.html"), "utf8");
    expect(html).not.toMatch(/<style[\s>]/i);
    for (const m of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
      expect(m[1], "scripts are files").toMatch(/\bsrc=/);
      expect(m[2].trim()).toBe("");
    }
    expect(html).not.toMatch(/\son[a-z]+=/i);
    expect(html).not.toMatch(/\sstyle=/i);
  });

  it("allows scripts from the app only", () => {
    const conf = JSON.parse(fs.readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"));
    const csp: string = conf.app.security.csp;
    const directive = (name: string) => csp.split(";").map((d) => d.trim()).find((d) => d.startsWith(`${name} `)) ?? "";
    expect(directive("script-src")).toBe("script-src 'self'");
    expect(csp).not.toContain("unsafe-eval");
    expect(directive("object-src")).toBe("object-src 'none'");
    expect(directive("connect-src")).not.toMatch(/https?:(?!\/\/ipc\.localhost)/);
    expect(conf.identifier).toBe("de.mousewerk.arcalo.mail");
  });

  it("lets the mail frame show what it allows: its srcdoc inherits the app's policy too", async () => {
    const conf = JSON.parse(fs.readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"));
    const img = /img-src([^;]*)/.exec(conf.app.security.csp)![1].trim().split(/\s+/);
    const { frameCsp } = await import("./mailframe");
    const frame = /img-src([^;]*)/.exec(frameCsp(true))![1].trim().split(/\s+/);
    // cid: pictures arrive as data: addresses (mail_core::sanitize); loaded pictures are web addresses.
    for (const scheme of frame.filter((s) => s !== "cid:")) expect(img).toContain(scheme);
  });
});
