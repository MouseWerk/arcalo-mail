// Text contrast over the theme tokens, as Arcalo's contrast test: every text color reaches 4.5:1
// (WCAG AA) on every surface it is shown on, in the dark, light and contrast themes of tokens.css.
// Tinted rows (hover, current, selected) are laid over the surface underneath. Muted text
// (--text-3: dates, counts, hints) reaches 4.5:1 on the plain surfaces and 4:1 on a highlighted
// row; the neutral focus ring reaches 3:1 everywhere. The contrast theme asks for 7:1 for text.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

type Rgb = [number, number, number];

function hex(h: string): Rgb | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(h.trim());
  if (!m) return null;
  const s = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1];
  return [0, 2, 4].map((i) => parseInt(s.slice(i, i + 2), 16)) as Rgb;
}

/** "rgb(r g b / a)" over a solid background; "#rrggbb" as it is. */
function solid(token: string, under: Rgb): Rgb | null {
  const m = /^rgb\((\d+) (\d+) (\d+)(?: \/ ([\d.]+))?\)$/.exec(token.trim());
  if (!m) return hex(token);
  const a = m[4] === undefined ? 1 : Number(m[4]);
  return [1, 2, 3].map((i, j) => Math.round(Number(m[i]) * a + under[j] * (1 - a))) as Rgb;
}

const lum = (c: Rgb) => {
  const [r, g, b] = c.map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a: Rgb, b: Rgb) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};

const SURFACES = ["--bg-canvas", "--bg-sidebar", "--bg-raised", "--bg-overlay", "--bg-input"];
const TINTS = ["--bg-hover", "--bg-current", "--bg-selected", "--accent-soft"];
const TEXT = ["--text", "--text-2", "--accent-text", "--success", "--warning", "--danger", "--info"];

/** The tokens of one theme block of tokens.css, `var(--x)` resolved within the block. */
function theme(selector: string): Record<string, string> {
  const css = fs.readFileSync(path.resolve(__dirname, "../styles/tokens.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const i = css.indexOf(selector);
  expect(i, selector).toBeGreaterThanOrEqual(0);
  const body = css.slice(i, css.indexOf("}", i));
  const k: Record<string, string> = Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
  for (const [name, v] of Object.entries(k)) {
    const ref = /^var\((--[\w-]+)\)$/.exec(v);
    if (ref && k[ref[1]]) k[name] = k[ref[1]];
  }
  return k;
}

/** Every text/background pair below the minimum, as readable lines. */
function failures(name: string, k: Record<string, string>, textMin = 4.5): string[] {
  const out: string[] = [];
  const color = (token: string, under: Rgb): Rgb | null => {
    const c = k[token] === undefined ? null : solid(k[token], under);
    if (!c) out.push(`${name}: ${token} is missing or not a color (${k[token]})`);
    return c;
  };
  const check = (fg: string, bgName: string, bg: Rgb | null, min: number) => {
    if (!bg) return;
    const f = color(fg, bg);
    if (!f) return;
    const c = contrast(f, bg);
    if (c < min) out.push(`${name}: ${fg} on ${bgName} ${c.toFixed(2)} < ${min}`);
  };
  for (const s of SURFACES) {
    const base = color(s, [0, 0, 0]);
    if (!base) continue;
    for (const fg of [...TEXT, "--text-3"]) check(fg, s, base, fg === "--text-3" ? 4.5 : textMin);
    for (const tint of TINTS) {
      const bg = color(tint, base);
      for (const fg of TEXT) check(fg, `${tint} on ${s}`, bg, 4.5);
      check("--text-3", `${tint} on ${s}`, bg, 4);
    }
    // Keyboard focus: the neutral ring 3:1 on every surface and on a current or selected row.
    check("--border-focus", s, base, 3);
    for (const tint of ["--bg-hover", "--bg-current", "--bg-selected"]) check("--border-focus", `${tint} on ${s}`, color(tint, base), 3);
  }
  // Status badges and banners: the status color on its own soft tint.
  for (const st of ["success", "warning", "danger", "info"]) {
    for (const s of ["--bg-canvas", "--bg-sidebar", "--bg-raised", "--bg-overlay"]) {
      const base = color(s, [0, 0, 0]);
      if (base) check(`--${st}`, `--${st}-soft on ${s}`, color(`--${st}-soft`, base), 4.5);
    }
  }
  // Primary buttons, switches, today's date in the calendar: --on-accent on the accent fill.
  check("--on-accent", "--accent-fill", color("--accent-fill", [0, 0, 0]), 4.5);
  return out;
}

describe("text contrast in every theme", () => {
  it("dark", () => {
    expect(failures("dark", theme(':root[data-theme="dark"] {'))).toEqual([]);
  });

  it("light", () => {
    expect(failures("light", theme(':root[data-theme="light"] {'))).toEqual([]);
  });

  it("contrast", () => {
    expect(failures("contrast", theme(':root[data-theme="contrast"] {'), 7)).toEqual([]);
  });
});
