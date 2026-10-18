// The release notes' shape (CLAUDE.md, as Arcalo's): docs/releases/v<version>.md has the sections
// "## Improved", "## Fixed" and "## Checklist for Windows (manual)" in this order and no others,
// a list item in each, and the version in the file name is x.y.z. The automatic release reads
// these files (scripts/release-pick.mjs); a malformed one fails here, before it releases.
//
//   node --test scripts/release-notes.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const DIR = path.join(ROOT, "docs/releases");
const SECTIONS = ["## Improved", "## Fixed", "## Checklist for Windows (manual)"];

/** What is wrong with one notes file (empty: nothing). */
export function problems(name, text) {
  const out = [];
  if (!/^v\d+\.\d+\.\d+\.md$/.test(name)) out.push(`${name}: name it v<major>.<minor>.<patch>.md`);
  const heads = text.split("\n").filter((l) => /^##\s/.test(l)).map((l) => l.trim());
  if (JSON.stringify(heads) !== JSON.stringify(SECTIONS)) out.push(`${name}: sections ${JSON.stringify(heads)}, expected ${JSON.stringify(SECTIONS)}`);
  for (const h of SECTIONS) {
    const at = text.indexOf(`${h}\n`);
    if (at < 0) continue;
    const body = text.slice(at + h.length).split(/\n## /)[0];
    if (!/^\s*[-*] \S/m.test(body)) out.push(`${name}: "${h}" has no list item`);
  }
  if (/\p{Emoji_Presentation}/u.test(text)) out.push(`${name}: no emojis`);
  return out;
}

test("every release notes file has the three sections", () => {
  const names = fs.existsSync(DIR) ? fs.readdirSync(DIR).filter((f) => f.endsWith(".md")) : [];
  assert.deepEqual(names.flatMap((n) => problems(n, fs.readFileSync(path.join(DIR, n), "utf8"))), []);
});

test("the check finds what it is meant to find", () => {
  const good = "# Arcalo Mail 1.1.0\n\n## Improved\n\n- A.\n\n## Fixed\n\n- B.\n\n## Checklist for Windows (manual)\n\n- [ ] C.\n";
  assert.deepEqual(problems("v1.1.0.md", good), []);
  assert.equal(problems("v1.1.md", good).length, 1);
  assert.equal(problems("v1.1.0.md", good.replace("## Fixed", "## Bug fixes")).length, 1);
  assert.equal(problems("v1.1.0.md", good.replace("- B.", "")).length, 1);
});
