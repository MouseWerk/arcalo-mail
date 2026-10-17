// The release's check of latest.json (scripts/verify-update-feed.mjs) with a throwaway key:
// node --test scripts/verify-update-feed.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { verifyFeed } from "./verify-update-feed.mjs";

/** A throwaway minisign key that signs prehashed, as the Tauri CLI does. */
function key() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
  const id = crypto.randomBytes(8);
  const pub = `untrusted comment: minisign public key\n${Buffer.concat([Buffer.from("Ed"), id, raw]).toString("base64")}\n`;
  return {
    pubkey: Buffer.from(pub).toString("base64"),
    sign(data, version) {
      const hash = crypto.createHash("blake2b512").update(data).digest();
      const sig = crypto.sign(null, hash, privateKey);
      const trusted = `timestamp:1790000000\tfile:x\tversion:${version}`;
      const global = crypto.sign(null, Buffer.concat([sig, Buffer.from(trusted)]), privateKey);
      const text = ["untrusted comment: sig", Buffer.concat([Buffer.from("ED"), id, sig]).toString("base64"), `trusted comment: ${trusted}`, global.toString("base64"), ""].join("\n");
      return Buffer.from(text).toString("base64");
    },
  };
}

function release(k, { version = "1.0.0", signedAs = version, tamper = false, tag = version } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-mail-feed-"));
  const file = "ArcaloMail_1.0.0_amd64.AppImage";
  const data = crypto.randomBytes(4096);
  fs.writeFileSync(path.join(dir, file), tamper ? Buffer.concat([data, Buffer.from("x")]) : data);
  const feed = {
    version,
    pub_date: "2026-11-01T00:00:00Z",
    platforms: { "linux-x86_64": { signature: k.sign(data, signedAs), url: `https://github.com/MouseWerk/arcalo-mail/releases/download/v${tag}/${file}` } },
  };
  fs.writeFileSync(path.join(dir, "latest.json"), JSON.stringify(feed));
  return dir;
}

test("a feed signed with the key of the release passes", () => {
  const k = key();
  const dir = release(k);
  const r = verifyFeed(dir, k.pubkey, "1.0.0");
  assert.ok(r.ok, r.lines.join("\n"));
  fs.rmSync(dir, { recursive: true });
});

test("another key, a changed file, another version or tag fail", () => {
  const k = key();
  for (const [what, dir, pub, version] of [
    ["another key", release(k), key().pubkey, "1.0.0"],
    ["a changed file", release(k, { tamper: true }), k.pubkey, "1.0.0"],
    ["signed for another version", release(k, { signedAs: "0.9.0" }), k.pubkey, "1.0.0"],
    ["another release", release(k), k.pubkey, "1.0.1"],
    ["the address of another tag", release(k, { tag: "0.9.0" }), k.pubkey, "1.0.0"],
  ]) {
    assert.equal(verifyFeed(dir, pub, version).ok, false, what);
    fs.rmSync(dir, { recursive: true });
  }
});
