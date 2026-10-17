// The automatic release's choice of version (scripts/release-pick.mjs): node --test scripts/release-pick.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { pick, parse } from "./release-pick.mjs";

const TAGS = ["v1.14.1", "v1.18.0", "v1.18.1", "screenshots-v1"];
const NOTES = ["v1.17.0.md", "v1.18.0.md", "v1.18.1.md", "highlights", "README.md"];

test("a notes file newer than every tag is released", () => {
  assert.equal(pick([...NOTES, "v1.19.0.md"], TAGS), "1.19.0");
  assert.equal(pick([...NOTES, "v1.18.2.md"], TAGS), "1.18.2");
  // Numbers compare as numbers, not as text.
  assert.equal(pick(["v1.9.0.md", "v1.10.0.md"], ["v1.9.0"]), "1.10.0");
  assert.equal(pick(["v1.0.0.md"], []), "1.0.0");
});

test("nothing is released twice or backwards", () => {
  assert.equal(pick(NOTES, TAGS), null, "the newest notes are tagged already");
  assert.equal(pick([...NOTES, "v1.18.0.md"], TAGS), null);
  // An older notes file that was never tagged does not count: only the newest one does.
  assert.equal(pick(["v1.16.5.md", ...NOTES], TAGS), null);
  // Lower than the newest tag (notes deleted or a tag pushed by hand).
  assert.equal(pick(["v1.17.0.md"], TAGS), null);
  // Its tag exists although older tags are missing from the list.
  assert.equal(pick(["v2.0.0.md"], ["v2.0.0"]), null);
  assert.equal(pick([], TAGS), null);
});

test("only plain release versions count", () => {
  assert.deepEqual(parse("v1.2.3"), [1, 2, 3]);
  for (const odd of ["1.2.3", "v1.2", "v1.2.3-rc.1", "v01.2.3", "screenshots-v1", "v1.2.3.4"]) assert.equal(parse(odd), null, odd);
  assert.equal(pick(["v1.20.0-rc.1.md", "v1.20.md", "notes.md"], TAGS), null);
  assert.equal(pick(["v1.19.0.md"], ["v1.20.0-rc.1", ...TAGS]), "1.19.0", "a pre-release tag is not a release");
});

test("the command line prints the version or nothing", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-release-pick-"));
  try {
    const notes = path.join(dir, "notes");
    fs.mkdirSync(notes);
    for (const f of ["v1.18.1.md", "v1.19.0.md"]) fs.writeFileSync(path.join(notes, f), "x");
    const tags = path.join(dir, "tags.txt");
    fs.writeFileSync(tags, TAGS.join("\n") + "\n");
    const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "release-pick.mjs");
    assert.equal(execFileSync(process.execPath, [script, notes, tags], { encoding: "utf8" }), "1.19.0\n");
    fs.appendFileSync(tags, "v1.19.0\n");
    assert.equal(execFileSync(process.execPath, [script, notes, tags], { encoding: "utf8" }), "");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
