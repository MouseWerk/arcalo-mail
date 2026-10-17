// Writes the release's version (from its tag) into the places the builds read it from: the
// workspace's Cargo.toml (the app's own version), src-tauri/tauri.conf.json (what the bundles,
// the updater and Android's versionName and versionCode take) and ui/package.json with its lock
// file. Used by release.yml, android.yml and ios.yml; the repository itself keeps 0.1.0.
//
//   VERSION=1.0.0 node scripts/set-version.mjs [root]      or      node scripts/set-version.mjs [root] 1.0.0

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const RELEASE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function setVersion(root, version) {
  if (!RELEASE.test(version)) throw new Error(`${version} is not MAJOR.MINOR.PATCH`);
  const cargo = path.join(root, "Cargo.toml");
  const toml = fs.readFileSync(cargo, "utf8");
  // The [workspace.package] version, the first `version =` at the start of a line.
  if (!/^version = "[^"]*"/m.test(toml)) throw new Error("no version in Cargo.toml");
  fs.writeFileSync(cargo, toml.replace(/^version = "[^"]*"/m, `version = "${version}"`));
  const conf = path.join(root, "src-tauri/tauri.conf.json");
  const c = JSON.parse(fs.readFileSync(conf, "utf8"));
  c.version = version;
  fs.writeFileSync(conf, `${JSON.stringify(c, null, 2)}\n`);
  for (const file of ["ui/package.json", "ui/package-lock.json"]) {
    const p = path.join(root, file);
    if (!fs.existsSync(p)) continue;
    const j = JSON.parse(fs.readFileSync(p, "utf8"));
    j.version = version;
    if (j.packages?.[""]) j.packages[""].version = version;
    fs.writeFileSync(p, `${JSON.stringify(j, null, 2)}\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const root = process.argv[2] ?? ".";
  const version = (process.argv[3] ?? process.env.VERSION ?? "").replace(/^v/, "");
  try {
    setVersion(root, version);
    console.log(`version ${version}`);
  } catch (e) {
    console.error(String(e.message ?? e));
    process.exit(1);
  }
}
