// Which version the automatic release (.github/workflows/auto-release.yml) publishes: the newest
// docs/releases/v<version>.md, when it is newer than every release tag and its tag does not exist
// yet. Prints that version (without v), or nothing.
//
//   node scripts/release-pick.mjs <notes folder> <file with one tag per line>

import fs from "node:fs";
import { pathToFileURL } from "node:url";

const RELEASE = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

/** `[major, minor, patch]` of `v1.2.3`, or null for anything else (pre-releases, `screenshots-v1`). */
export function parse(tag) {
  const m = RELEASE.exec(tag.trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

export function compare(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return 0;
}

/**
 * The version to release from the release-note file names (`v1.19.0.md`) and the existing tags,
 * or null: only the newest notes file counts, it must be newer than the newest release tag, and its
 * own tag must not exist.
 */
export function pick(noteFiles, tags) {
  const notes = noteFiles
    .map((f) => (f.endsWith(".md") ? parse(f.slice(0, -3)) : null))
    .filter(Boolean)
    .sort(compare);
  const newest = notes.at(-1);
  if (!newest) return null;
  const tagged = tags.map((t) => t.trim()).filter(Boolean);
  const released = tagged.map(parse).filter(Boolean).sort(compare).at(-1);
  if (released && compare(newest, released) <= 0) return null;
  const version = newest.join(".");
  if (tagged.includes(`v${version}`)) return null;
  return version;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [dir, tagFile] = process.argv.slice(2);
  if (!dir || !tagFile) {
    console.error("usage: node scripts/release-pick.mjs <notes folder> <tag file>");
    process.exit(2);
  }
  const version = pick(fs.readdirSync(dir), fs.readFileSync(tagFile, "utf8").split("\n"));
  if (version) console.log(version);
}
