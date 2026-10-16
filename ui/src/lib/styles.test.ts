// Design rules of CLAUDE.md that a machine can check: colors only as tokens (tokens.css), no
// accent for selected, current or focused states, no colored bar at the side, and focus rings
// only on :focus-visible.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

const DIR = path.resolve(__dirname, "../styles");
const sheets = fs.readdirSync(DIR).filter((f) => f.endsWith(".css"));
const read = (f: string) => fs.readFileSync(path.join(DIR, f), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The rules of a stylesheet as [selector, body] (one level of @media nesting is enough here). */
function rules(css: string): [string, string][] {
  const out: [string, string][] = [];
  for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) out.push([m[1].trim(), m[2]]);
  return out;
}

describe("styles", () => {
  it("uses colors from tokens.css only", () => {
    const literal = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/;
    const bad = sheets
      .filter((f) => f !== "tokens.css")
      .flatMap((f) => read(f).split("\n").map((l, i) => [f, i + 1, l] as const))
      .filter(([, , l]) => literal.test(l))
      .map(([f, i, l]) => `${f}:${i}: ${l.trim()}`);
    expect(bad).toEqual([]);
  });

  it("marks selected, current and focused items without the accent", () => {
    const state = /aria-current|aria-selected|\[data-selected|:focus|\.selected|\.current|\.active/;
    const bad = sheets.flatMap((f) =>
      rules(read(f))
        .filter(([sel, body]) => state.test(sel) && /--accent|--unread/.test(body))
        .map(([sel]) => `${f}: ${sel}`),
    );
    expect(bad).toEqual([]);
  });

  it("draws no colored bar at the side", () => {
    const bad = sheets.flatMap((f) =>
      rules(read(f))
        .filter(([, body]) => /border-(left|inline-start)\s*:[^;]*(--accent|--info|--success|--warning|--danger|--violet)/.test(body))
        .map(([sel]) => `${f}: ${sel}`),
    );
    expect(bad).toEqual([]);
  });

  it("shows focus rings on :focus-visible only", () => {
    const bad = sheets.flatMap((f) =>
      rules(read(f))
        .filter(([sel, body]) => /:focus(?!-visible|-within)/.test(sel) && /outline\s*:(?!\s*none)/.test(body))
        .map(([sel]) => `${f}: ${sel}`),
    );
    expect(bad).toEqual([]);
  });

  it("defines every token in the dark, light and contrast themes", () => {
    const css = read("tokens.css");
    const block = (sel: string) => {
      const i = css.indexOf(sel);
      return css.slice(i, css.indexOf("}", i));
    };
    const names = (b: string) => [...b.matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1]).sort();
    const dark = names(block(':root[data-theme="dark"] {'));
    expect(dark.length).toBeGreaterThan(40);
    expect(names(block(':root[data-theme="light"] {'))).toEqual(dark);
    expect(names(block(':root[data-theme="contrast"] {'))).toEqual(dark);
  });

  it("uses only tokens that exist", () => {
    const defined = new Set([...read("tokens.css").matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1]));
    for (const f of sheets) for (const m of read(f).matchAll(/(--[\w-]+)\s*:/g)) defined.add(m[1]);
    const used = sheets.flatMap((f) => [...read(f).matchAll(/var\((--[\w-]+)/g)].map((m) => `${f}: ${m[1]}`));
    expect(used.filter((u) => !defined.has(u.split(": ")[1]))).toEqual([]);
  });
});
