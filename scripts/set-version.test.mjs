// The release's version in Cargo.toml, tauri.conf.json and the UI's package files
// (scripts/set-version.mjs): node --test scripts/set-version.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { setVersion } from "./set-version.mjs";

test("the version goes into every file the builds read", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-version-"));
  try {
    fs.mkdirSync(path.join(root, "src-tauri"));
    fs.mkdirSync(path.join(root, "ui"));
    fs.writeFileSync(path.join(root, "Cargo.toml"), '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\nx = { version = "1" }\n');
    fs.writeFileSync(path.join(root, "src-tauri/tauri.conf.json"), JSON.stringify({ productName: "Arcalo Mail", version: "0.1.0" }));
    fs.writeFileSync(path.join(root, "ui/package.json"), JSON.stringify({ name: "ui", version: "0.1.0" }));
    fs.writeFileSync(path.join(root, "ui/package-lock.json"), JSON.stringify({ name: "ui", version: "0.1.0", packages: { "": { version: "0.1.0" } } }));
    setVersion(root, "1.0.0");
    const toml = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
    assert.match(toml, /^version = "1\.0\.0"$/m);
    assert.match(toml, /x = \{ version = "1" \}/, "dependencies keep theirs");
    assert.equal(JSON.parse(fs.readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8")).version, "1.0.0");
    assert.equal(JSON.parse(fs.readFileSync(path.join(root, "ui/package.json"), "utf8")).version, "1.0.0");
    assert.equal(JSON.parse(fs.readFileSync(path.join(root, "ui/package-lock.json"), "utf8")).packages[""].version, "1.0.0");
    assert.throws(() => setVersion(root, "1.0"), /MAJOR\.MINOR\.PATCH/);
    assert.throws(() => setVersion(root, "1.0.0-rc.1"), /MAJOR\.MINOR\.PATCH/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("the repository's files have the shape the script expects", () => {
  const root = path.resolve(import.meta.dirname, "..");
  assert.match(fs.readFileSync(path.join(root, "Cargo.toml"), "utf8"), /^\[workspace\.package\]\nversion = "[^"]+"/m);
  assert.ok(JSON.parse(fs.readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8")).version);
});
