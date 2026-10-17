// One notation for key combinations, as in Arcalo (shortcut.test.ts there, q116 R8): keys are
// joined with + and no spaces („Strg+Umschalt+K“), so tooltips, menus and hint texts read the same
// and split into key chips the same way.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";

const KEY = "Strg|Ctrl|Umschalt|Shift|Alt|Option|Cmd|Eingabe|Enter|Entf|Del|Tab|Esc";

describe("key combinations in the catalogs", () => {
  for (const lang of ["de", "en"]) {
    it(`${lang}: written with + and without spaces`, () => {
      const src = fs.readFileSync(path.resolve(__dirname, `../locales/${lang}.ts`), "utf8");
      const spaced = src.match(new RegExp(`\\b(Strg|Ctrl|Umschalt|Shift) (${KEY}|[A-Z0-9])\\b`, "g")) ?? [];
      const padded = src.match(new RegExp(`\\b(${KEY}) \\+ ?(${KEY}|[A-Z0-9,.])(?![\\w])`, "g")) ?? [];
      expect([...spaced, ...padded]).toEqual([]);
    });
  }
});
