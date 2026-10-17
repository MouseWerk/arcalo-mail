// The app draws its own controls: the system's stock ones (a `<select>`'s dropdown, the date and
// time pickers, checkboxes, sliders, file fields, `alert`/`confirm`/`prompt`) look like Android,
// Windows or GTK instead of the app, and a phone shows them as foreign dialogs. Use `Select`,
// `DateTimePicker`, `Switch`, `Segmented`, `ConfirmDialog`/`PromptDialog` and `pickFiles` instead.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

const SRC = path.resolve(__dirname, "..");

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "tests" ? [] : sources(p);
    return /\.(tsx?|jsx?)$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });
}

const found = (re: RegExp) =>
  sources(SRC).flatMap((f) =>
    fs
      .readFileSync(f, "utf8")
      .split("\n")
      // Code only: comments may name what they replace.
      .flatMap((line, i) => (!/^\s*(\/\/|\/?\*)/.test(line) && re.test(line) ? [`${path.relative(SRC, f)}:${i + 1}: ${line.trim()}`] : [])),
  );

describe("the app's own controls", () => {
  it("draws no system select", () => {
    expect(found(/<select[\s>]/)).toEqual([]);
  });

  it("draws no system date or time picker", () => {
    expect(found(/type=["'{]+(date|time|datetime-local|month|week)["'}]/)).toEqual([]);
  });

  it("draws no system checkbox, radio, slider, progress bar, color or file field", () => {
    expect(found(/type=["'{]+(checkbox|radio|range|color|file)["'}]|<progress[\s>]|<meter[\s>]/)).toEqual([]);
  });

  it("asks no question in a system dialog", () => {
    expect(found(/(^|[^.\w])(window\.)?(alert|confirm|prompt)\s*\(/)).toEqual([]);
    expect(found(/window\.(alert|confirm|prompt)\b/)).toEqual([]);
  });
});
