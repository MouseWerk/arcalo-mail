// Mail HTML is shown only in the sandboxed frame (CLAUDE.md): sanitized in the core, rendered by
// components/MailFrame.tsx through lib/mailframe.ts with its own CSP, never with `allow-scripts`.
// This test fails on a second place that renders HTML (srcdoc, dangerouslySetInnerHTML, an
// innerHTML assignment, insertAdjacentHTML, document.write) and on a sandbox that lets scripts
// run. The few places that set HTML on purpose are listed with the reason.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { FRAME_SANDBOX, frameDocument } from "./mailframe";

const SRC = path.resolve(__dirname, "..");

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "tests" || e.name === "locales" ? [] : sources(p);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });
}

/** `file` (relative to ui/src) and a pattern of the line that may set HTML, with the reason. */
const ALLOWED: { file: string; line: RegExp; why: string }[] = [
  { file: "components/MailFrame.tsx", line: /frame\.srcdoc = frameDocument\(/, why: "the one place mail HTML is shown: the sandboxed frame" },
  {
    file: "components/ComposeSheet.tsx",
    line: /ref\.current\.innerHTML = html/,
    why: "the compose editor (contentEditable): the draft's own HTML, quotes sanitized by the core (sanitize_html_inline); the app's CSP blocks inline handlers",
  },
];

const SETS_HTML = /\.srcdoc\s*=|dangerouslySetInnerHTML|\.innerHTML\s*=(?!=)|\.outerHTML\s*=(?!=)|insertAdjacentHTML\(|document\.write\(/;

describe("mail HTML", () => {
  it("is rendered only in the sandboxed frame", () => {
    const bad: string[] = [];
    for (const f of sources(SRC)) {
      const rel = path.relative(SRC, f).split(path.sep).join("/");
      fs.readFileSync(f, "utf8")
        .split("\n")
        .forEach((l, i) => {
          if (!SETS_HTML.test(l) || /^\s*(\/\/|\*|\/\*)/.test(l)) return;
          if (!ALLOWED.some((a) => a.file === rel && a.line.test(l))) bad.push(`${rel}:${i + 1}: ${l.trim().slice(0, 100)}`);
        });
    }
    expect(bad).toEqual([]);
  });

  it("every listed place is still there (the list stays honest)", () => {
    for (const a of ALLOWED) expect(fs.readFileSync(path.join(SRC, a.file), "utf8"), a.file).toMatch(a.line);
  });

  it("runs no scripts: no allow-scripts in the sandbox or anywhere in the code", () => {
    expect(FRAME_SANDBOX.split(/\s+/)).not.toContain("allow-scripts");
    expect(FRAME_SANDBOX.split(/\s+/)).not.toContain("allow-top-navigation");
    const bad = sources(SRC).filter((f) => fs.readFileSync(f, "utf8").split("\n").some((l) => /allow-scripts/.test(l) && !/^\s*(\/\/|\*|\/\*)/.test(l)));
    expect(bad.map((f) => path.relative(SRC, f))).toEqual([]);
  });

  it("the frame's own CSP forbids scripts", () => {
    const doc = frameDocument("<p>x</p><script>1</script>", false, { text: "#000", background: "#fff", link: "#00f" } as never);
    const csp = /Content-Security-Policy" content="([^"]+)"/.exec(doc)?.[1] ?? "";
    expect(csp).toMatch(/script-src 'none'|default-src 'none'/);
    expect(csp).not.toMatch(/unsafe-inline[^;]*;?\s*$|script-src[^;]*unsafe/);
  });
});
