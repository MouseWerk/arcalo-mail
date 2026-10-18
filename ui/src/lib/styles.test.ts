// Design rules of CLAUDE.md that a machine can check, taken from Arcalo's styles test
// (ui/src/styles/styles.test.ts there) and adapted: colors only as tokens (tokens.css), weights,
// layers and radii from the tokens, no accent for selected, current or focused states, no glow or
// gradient frame, no colored bar at the side, focus rings only on :focus-visible, no faded text,
// endless animations only of opacity and transform, and stylesheets a merge cannot silently break.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

const DIR = path.resolve(__dirname, "../styles");
const SRC = path.resolve(__dirname, "..");
const sheets = fs.readdirSync(DIR).filter((f) => f.endsWith(".css"));
const raw = (f: string) => fs.readFileSync(path.join(DIR, f), "utf8");
// Comments become blank lines, so line numbers stay right.
const read = (f: string) => raw(f).replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ""));

/** The rules of a stylesheet as [selector, body] (one level of @media nesting is enough here). */
function rules(css: string): [string, string, number][] {
  const out: [string, string, number][] = [];
  for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const line = css.slice(0, (m.index ?? 0) + m[1].length - m[1].trimStart().length).split("\n").length;
    out.push([m[1].replace(/\s+/g, " ").trim(), m[2], line]);
  }
  return out;
}

/** The rules with their declarations split into [property, value]. */
const decls = (f: string) =>
  rules(read(f)).map(([selector, body, line]) => ({
    selector,
    at: `${f}:${line}`,
    decls: body
      .split(";")
      .filter((d) => d.includes(":"))
      .map((d) => [d.slice(0, d.indexOf(":")).trim(), d.slice(d.indexOf(":") + 1).trim()] as const),
  }));

describe("stylesheets", () => {
  // The stylesheets are bundled into one file: an unclosed rule in one of them silently swallows
  // every rule after it (a merge in Arcalo once dropped a `}` and took a whole sheet with it).
  for (const name of sheets) {
    it(`${name} has balanced braces and no merge markers`, () => {
      const css = raw(name);
      expect(css).not.toMatch(/^(<<<<<<<|=======|>>>>>>>)/m);
      const code = css.replace(/\/\*[\s\S]*?\*\//g, "").replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/g, "");
      let depth = 0;
      for (const ch of code) {
        if (ch === "{") depth++;
        else if (ch === "}") depth--;
        expect(depth).toBeGreaterThanOrEqual(0);
      }
      expect(depth).toBe(0);
    });
  }
});

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
        .map(([sel, , line]) => `${f}:${line}: ${sel}`),
    );
    expect(bad).toEqual([]);
  });

  it("draws no colored bar at the side", () => {
    const bad = sheets.flatMap((f) =>
      rules(read(f))
        .filter(([, body]) => /border-(left|inline-start)\s*:[^;]*(--accent|--info|--success|--warning|--danger|--violet)/.test(body))
        .map(([sel, , line]) => `${f}:${line}: ${sel}`),
    );
    expect(bad).toEqual([]);
  });

  it("shows focus rings on :focus-visible only", () => {
    // A ring drawn on plain :focus shows on every mouse click too: outline and box-shadow belong to
    // :focus-visible (or :focus-within for a field group). `outline: none` is fine anywhere.
    const bad = sheets.flatMap((f) =>
      rules(read(f))
        .filter(([sel]) => sel.split(",").some((s) => /:focus(?!-visible|-within)/.test(s)))
        .filter(([, body]) => /(^|;)\s*outline\s*:(?!\s*(none|0)\s*(;|$))/.test(body) || /(^|;)\s*box-shadow\s*:(?!\s*none)/.test(body))
        .map(([sel, , line]) => `${f}:${line}: ${sel}`),
    );
    expect(bad, "use :focus-visible").toEqual([]);
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

// Non-color design tokens: weights, layers, the focus ring, eyebrow labels and pill radii come from
// one place, so a view cannot drift to its own values.
describe("design tokens", () => {
  for (const name of sheets.filter((f) => f !== "tokens.css")) {
    it(`${name} takes weights, layers, focus ring and pill radii from the tokens`, () => {
      const css = read(name);
      expect(css.match(/font-weight:\s*\d{3}\b[^;}]*/g) ?? []).toEqual([]);
      expect(css.match(/z-index:\s*\d{2,}[^;}]*/g) ?? []).toEqual([]);
      expect(css.match(/outline:\s*2px solid var\(--border-focus\)/g) ?? []).toEqual([]);
      expect(css.match(/border-radius:[^;}]*\b999px/g) ?? []).toEqual([]);
    });

    it(`${name} sets the mono font without ligatures`, () => {
      // Contextual alternates of the mono font draw „http://“ as „http: //“ and „!==“ as one glyph.
      const odd = rules(read(name))
        .filter(([, body]) => /(^|;)\s*font(-family)?:[^;]*var\(--font-mono\)/.test(body) && !/font-variant-ligatures:\s*none/.test(body))
        .map(([sel, , line]) => `${name}:${line}: ${sel}`);
      expect(odd, "add font-variant-ligatures: none").toEqual([]);
    });

    it(`${name} styles uppercase section labels one way`, () => {
      const odd = rules(read(name))
        .filter(([, body]) => /text-transform:\s*uppercase/.test(body) && /letter-spacing:\s*0\.04em/.test(body))
        .map(([sel, , line]) => `${name}:${line}: ${sel}`);
      expect(odd, "use var(--ls-eyebrow)").toEqual([]);
    });
  }
});

// Selection, focus and side bars (CLAUDE.md): selected, active and current items are a neutral
// tint with text weight or color, never an accent border, outline, ring or glow; keyboard focus is
// a neutral ring on :focus-visible; nothing has a colored bar at its left side (a plain blockquote
// keeps a thin gray line). A filled control (a checked box) may keep a border in its own fill
// color, and drop targets while dragging may use the accent.
describe("selection and focus look", () => {
  const STATE =
    /\.(active|selected|sel|current|on|checked|picked|now|today|focused|is-selected|is-editing|recording|running|live|playing|unread|open|expanded)\b|:checked|:focus(-visible|-within)?\b|\[aria-(selected|current|pressed|checked|expanded)|\[data-(selected|active|current)/;
  // The accent and the status colors: a red frame around a running thing is the same "AI look".
  const ACCENT = /--(accent(-strong|-soft|-text)?|unread|success|warning|danger|info)\b/;
  const FRAME = /^(border(-(top|right|bottom|left|inline|block|inline-start|inline-end))?(-color)?|outline(-color)?|box-shadow)$/;
  const DRAG = /drop|drag|\.over\b/;
  const COLORED = /--(accent|unread|success|warning|danger|info|violet|ev|c|c-fill|star|series-\d|cal-[\w-]+)\b|color-mix|#[0-9a-f]{3,8}\b|rgba?\(/i;
  const SIDE = /^border-(left|inline-start)$/;
  const looked = sheets.filter((f) => f !== "tokens.css");

  for (const name of looked) {
    it(`${name}: selected, current and focused states have no accent frame`, () => {
      const bad: string[] = [];
      for (const { selector, at, decls: ds } of decls(name)) {
        if (!STATE.test(selector) || DRAG.test(selector) || /^(@|from|to|\d)/.test(selector)) continue;
        const fill = ds.find(([p]) => p === "background" || p === "background-color")?.[1] ?? "";
        for (const [p, v] of ds) {
          if (!FRAME.test(p) || !ACCENT.test(v)) continue;
          // A filled control: the border is its own fill.
          if (/^border(-color)?$/.test(p) && fill && v === fill) continue;
          bad.push(`${at}: ${selector} { ${p}: ${v} }`);
        }
      }
      expect(bad, "use --bg-selected/--bg-current and --focus-ring").toEqual([]);
    });

    it(`${name}: no glow or gradient frame marks a state`, () => {
      const bad: string[] = [];
      for (const { selector, at, decls: ds } of decls(name)) {
        if (!STATE.test(selector) || DRAG.test(selector)) continue;
        for (const [p, v] of ds) {
          // A colored blur around an item (a glow), or a gradient painted as its frame.
          const tinted = v.replace(/rgb\(0 0 0 \/ [\d.]+\)/g, "");
          if (p === "box-shadow" && !/inset/.test(v) && /\b\d+(px)?\s+-?\d+(px)?\s+[1-9]\d*px/.test(v) && COLORED.test(tinted)) bad.push(`${at}: ${selector} { ${p}: ${v} }`);
          if (/^border(-image)?(-source)?$/.test(p) && /gradient\(/.test(v)) bad.push(`${at}: ${selector} { ${p}: ${v} }`);
          if (/^background(-image)?$/.test(p) && /gradient\(/.test(v) && /border-box/.test(v)) bad.push(`${at}: ${selector} { ${p}: ${v} }`);
        }
      }
      expect(bad, "use a neutral tint and the text weight").toEqual([]);
    });

    it(`${name}: no colored bar at the left side`, () => {
      const bad: string[] = [];
      for (const { selector, at, decls: ds } of decls(name)) {
        if (DRAG.test(selector)) continue;
        const get = (p: string) => ds.find(([q]) => q === p)?.[1] ?? "";
        for (const [p, v] of ds) {
          const line = `${at}: ${selector} { ${p}: ${v} }`;
          if (/^border-(left|inline-start)(-color)?$/.test(p) && COLORED.test(v)) bad.push(line);
          // A thick side line is a bar even in gray; only blockquotes keep a thin one.
          else if (SIDE.test(p) && /\b([3-9]|\d{2,})px\b/.test(v)) bad.push(line);
          else if (SIDE.test(p) && /\b2px\b/.test(v) && !/blockquote/.test(selector)) bad.push(line);
          // An inset shadow offset sideways draws a bar.
          if (p === "box-shadow" && /inset\s+-?[1-9]\d*(px)?\s+0\b/.test(v)) bad.push(line);
        }
        // A thin pseudo element along the left edge in a color.
        if (/::(before|after)/.test(selector) && get("left") === "0" && /^[1-4]px$/.test(get("width")) && COLORED.test(get("background"))) {
          bad.push(`${at}: ${selector} (side bar)`);
        }
        // A thin upright element in a color in front of a row (a calendar color as a bar instead of
        // a dot), sized by itself or by a 1-4 px first grid column. A marker placed inside a chart
        // (the „now“ line of a day) is no side bar.
        const marker = get("position") === "absolute" && !/^0(px)?$/.test(get("left") || "0");
        const fill = get("background") || get("background-color");
        const px = (v: string) => (/^\d+(\.\d+)?px$/.test(v) ? parseFloat(v) : null);
        const w = px(get("width"));
        const h = px(get("height"));
        if (!marker && COLORED.test(fill) && h != null && h >= 8 && ((w != null && w <= 4) || (!get("width") && /(bar|stripe|rail|edge)$/.test(selector)))) {
          bad.push(`${at}: ${selector} (upright bar, use an 8 px dot)`);
        }
        if (/^[1-4]px\b/.test(get("grid-template-columns"))) bad.push(`${at}: ${selector} { grid-template-columns: ${get("grid-template-columns")} } (a bar column)`);
      }
      expect(bad, "use a tinted background and an icon").toEqual([]);
    });

    it(`${name}: no colored ring marks a state`, () => {
      const bad: string[] = [];
      for (const { selector, at, decls: ds } of decls(name)) {
        // A slider's thumb is a control filled with its own color.
        if (DRAG.test(selector) || /slider-thumb|range-thumb/.test(selector)) continue;
        for (const [p, v] of ds) {
          if (p === "box-shadow" && /inset\s+0(px)?\s+0(px)?\s+0(px)?\s+[1-9]/.test(v) && ACCENT.test(v)) bad.push(`${at}: ${selector} { ${p}: ${v} }`);
        }
      }
      expect(bad, "use a tinted background and text color").toEqual([]);
    });
  }

  // Readable content is not faded with opacity (that drops it below 4.5:1): a state shows by the
  // muted text color and a mark. Opacity stays for disabled controls, icons, drag and loading
  // states and drawn decoration.
  const FADE_OK = [
    /:disabled|\.disabled\b|\[disabled\]|\[aria-disabled/,
    /drag|is-loading|loading|skeleton|splash|\.leaving\b|\.entering\b/,
    /icon|svg|chevron|caret|dot\b|marker|faint-mark|meter|handle|resize|progress|spinner/,
    /btn|button|\.act\b|actions\b|swipe/,
  ];
  for (const name of looked) {
    it(`${name}: no faded text`, () => {
      const bad: string[] = [];
      for (const { selector, at, decls: ds } of decls(name)) {
        if (/^(@|from|to|\d)/.test(selector) || FADE_OK.some((re) => re.test(selector))) continue;
        for (const [p, v] of ds) if (p === "opacity" && /^0?\.[0-7]\d*$/.test(v)) bad.push(`${at}: ${selector} { opacity: ${v} }`);
      }
      expect(bad, "use color: var(--text-3) and a mark instead").toEqual([]);
    });
  }

  it("inline styles in components draw no side bars or accent frames", () => {
    const files = (d: string): string[] =>
      fs
        .readdirSync(d, { withFileTypes: true })
        .flatMap((e) => (e.isDirectory() ? files(path.join(d, e.name)) : /\.tsx$/.test(e.name) && !/\.test\./.test(e.name) ? [path.join(d, e.name)] : []));
    const bad: string[] = [];
    for (const f of files(SRC)) {
      const src = fs.readFileSync(f, "utf8");
      for (const m of src.matchAll(/\b(borderLeft\w*|borderInlineStart\w*|outline\w*|boxShadow|borderColor)\s*:\s*([^,}\n]+)/g)) {
        if (/^border(Left|InlineStart)/.test(m[1]) || ACCENT.test(m[2])) {
          const line = src.slice(0, m.index).split("\n").length;
          bad.push(`${path.relative(SRC, f)}:${line}: ${m[0]}`);
        }
      }
    }
    expect(bad).toEqual([]);
  });

  it("focus and selection tokens are neutral", () => {
    const css = read("tokens.css");
    for (const token of ["--border-focus", "--focus-ring", "--focus-halo", "--focus-soft", "--bg-selected", "--bg-current", "--selected-ring", "--current-ring"]) {
      const values = [...css.matchAll(new RegExp(`${token}:\\s*([^;]+);`, "g"))].map((m) => m[1]);
      expect(values.length, token).toBeGreaterThan(0);
      for (const v of values) expect(v, token).not.toMatch(ACCENT);
    }
  });
});

// An endless animation of anything but opacity or transform repaints every frame and keeps a CPU
// core busy (a box-shadow pulse in Arcalo did that for as long as a timer ran).
const ENDLESS_PAINT_OK: [string, string][] = [];

describe("endless animations", () => {
  const css = sheets.map(read).join("\n");
  const keyframes = new Map<string, string>();
  for (const m of css.matchAll(/@keyframes\s+([\w-]+)\s*\{((?:[^{}]*\{[^{}]*\})*[^{}]*)\}/g)) keyframes.set(m[1], m[2]);
  const all = [...css.matchAll(/([^{}@]+)\{([^{}]*)\}/g)].map((m) => ({ selector: m[1].replace(/\s+/g, " ").trim(), body: m[2] }));
  const endless = all.flatMap(({ selector, body }) =>
    [...body.matchAll(/animation(?:-name)?\s*:\s*([^;]+)/g)]
      .map((a) => a[1])
      .filter((v) => /\binfinite\b/.test(v))
      .map((v) => ({ selector, name: v.split(/[\s,]+/).find((w) => keyframes.has(w)) ?? "" })),
  );

  it("are found by the check", () => {
    expect(keyframes.size).toBeGreaterThan(2);
    expect(endless.length).toBeGreaterThan(0);
  });

  it("loop only opacity and transform", () => {
    const bad: string[] = [];
    for (const { selector, name } of endless) {
      if (ENDLESS_PAINT_OK.some(([n]) => n === name)) continue;
      const props = [...(keyframes.get(name) ?? "?:").matchAll(/([\w-]+)\s*:/g)].map((p) => p[1]);
      if (!name || props.some((p) => p !== "opacity" && p !== "transform")) bad.push(`${selector} (${name || "unknown keyframes"})`);
    }
    expect(bad).toEqual([]);
  });

  it("stand still with reduced motion", () => {
    expect(css).toMatch(/@media\s*\(prefers-reduced-motion:\s*reduce\)/);
  });
});
