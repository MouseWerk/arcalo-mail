// Verifies an update feed before the release is published (release.yml): latest.json names the
// version, every platform's address points at this release's tag, and every signature (minisign,
// Ed25519 over the BLAKE2b-512 hash of the file, the trusted comment naming the version) verifies
// against the public key of tauri.conf.json. From the guidelines' templates/verify-update-feed.mjs.
//
//   node scripts/verify-update-feed.mjs <dir with latest.json and the files> <public key (base64)> <version>

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** Checks the feed in `dir`; returns one line per platform and whether all of them are good. */
export function verifyFeed(dir, pubkeyB64, version) {
  const pubLine = Buffer.from(pubkeyB64.trim(), "base64").toString().split("\n")[1] ?? "";
  const pk = Buffer.from(pubLine, "base64");
  const keyId = pk.subarray(2, 10);
  const key = crypto.createPublicKey({
    key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pk.subarray(10)]),
    format: "der",
    type: "spki",
  });
  const feed = JSON.parse(fs.readFileSync(path.join(dir, "latest.json"), "utf8"));
  const lines = [`version ${feed.version} pub_date ${feed.pub_date}`];
  let ok = feed.version === version;
  if (!ok) lines.push(`the feed names ${feed.version}, the release is ${version}`);
  const platforms = Object.entries(feed.platforms ?? {});
  if (!platforms.length) {
    lines.push("no platform");
    ok = false;
  }
  for (const [platform, { signature, url }] of platforms) {
    if (!url.includes(`/v${version}/`)) {
      lines.push(`${platform} wrong url ${url}`);
      ok = false;
      continue;
    }
    const file = path.join(dir, decodeURIComponent(url.split("/").pop()));
    const sigLines = Buffer.from(signature, "base64").toString().split("\n");
    const sig = Buffer.from(sigLines[1] ?? "", "base64");
    const trusted = (sigLines[2] ?? "").replace(/^trusted comment: /, "");
    const global = Buffer.from(sigLines[3] ?? "", "base64");
    const okId = sig.subarray(2, 10).equals(keyId);
    // Prehashed (the Tauri CLI signs so): Ed25519 over BLAKE2b-512 of the file.
    const hash = crypto.createHash("blake2b512").update(fs.readFileSync(file)).digest();
    const okSig = crypto.verify(null, hash, key, sig.subarray(10));
    const okGlobal = crypto.verify(null, Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), key, global);
    const okVersion = trusted.split("\t").includes(`version:${version}`);
    const good = okId && okSig && okGlobal && okVersion;
    lines.push(`${platform} ${path.basename(file)} ${good ? "OK" : `BAD id=${okId} sig=${okSig} global=${okGlobal} version=${okVersion}`}`);
    if (!good) ok = false;
  }
  return { ok, lines };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [dir, pubkey, version] = process.argv.slice(2);
  if (!dir || !pubkey || !version) {
    console.error("usage: node scripts/verify-update-feed.mjs <dir> <public key (base64)> <version>");
    process.exit(2);
  }
  const { ok, lines } = verifyFeed(dir, pubkey, version);
  for (const l of lines) console.log(l);
  process.exit(ok ? 0 : 1);
}
