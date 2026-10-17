// The signing certificate's digest of an APK (scripts/apk-signer.mjs), on APKs built here: a ZIP with
// an APK Signing Block holding a v2 and/or v3 signer. node --test scripts/apk-signer.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isDebugSubject, signerDigest, signerSubject } from "./apk-signer.mjs";

const u32 = (n) => {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
};
const u64 = (n) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
};
const lp = (...parts) => {
  const body = Buffer.concat(parts);
  return Buffer.concat([u32(body.length), body]);
};

/** A v2/v3 scheme block with one signer and its certificates (as the format nests them). */
function scheme(certs, v3 = false) {
  const digests = lp(lp(u32(0x0103), lp(Buffer.from("digest"))));
  const certificates = lp(...certs.map((c) => lp(c)));
  const signedData = lp(digests, certificates, ...(v3 ? [u32(28), u32(0x7fffffff)] : []), lp());
  const signer = lp(signedData, ...(v3 ? [u32(28), u32(0x7fffffff)] : []), lp(), lp(Buffer.from("key")));
  return lp(signer);
}

/** A ZIP with one stored file and an APK Signing Block of `pairs` ([id, value]) before its central directory. */
function apk(pairs, comment = "") {
  const name = Buffer.from("AndroidManifest.xml");
  const data = Buffer.from("manifest");
  const local = Buffer.concat([u32(0x04034b50), Buffer.alloc(22), Buffer.from([name.length, 0, 0, 0]), name, data]);
  let block = Buffer.alloc(0);
  if (pairs) {
    const body = Buffer.concat(pairs.map(([id, value]) => Buffer.concat([u64(value.length + 4), u32(id), value])));
    const size = body.length + 8 + 16;
    block = Buffer.concat([u64(size), body, u64(size), Buffer.from("APK Sig Block 42")]);
  }
  const cdStart = local.length + block.length;
  const central = Buffer.concat([u32(0x02014b50), Buffer.alloc(24), Buffer.from([name.length, 0]), Buffer.alloc(16), name]);
  const eocd = Buffer.concat([u32(0x06054b50), Buffer.alloc(4), Buffer.from([1, 0, 1, 0]), u32(central.length), u32(cdStart), Buffer.from([comment.length, 0]), Buffer.from(comment)]);
  return Buffer.concat([local, block, central, eocd]);
}

const sha = (b) => crypto.createHash("sha256").update(b).digest("hex");
const certA = Buffer.from("0\x82 certificate A (DER)");
const certB = Buffer.from("0\x82 certificate B (DER)");

test("the first certificate of the v2 signer", () => {
  assert.equal(signerDigest(apk([[0x7109871a, scheme([certA, certB])]])), sha(certA));
  // Other pairs (padding, dependency info) and a ZIP comment do not disturb it.
  const padded = apk([[0x42726577, Buffer.alloc(100)], [0x7109871a, scheme([certA])], [0x504b4453, Buffer.from("deps")]], "a comment");
  assert.equal(signerDigest(padded), sha(certA));
});

test("v3 comes before v2 (the current signer after a key rotation)", () => {
  const both = apk([[0x7109871a, scheme([certA])], [0xf05368c0, scheme([certB], true)]]);
  assert.equal(signerDigest(both), sha(certB));
  assert.equal(signerDigest(apk([[0xf05368c0, scheme([certB], true)]])), sha(certB));
});

test("no digest without a v2 or v3 signer, or from a broken block", () => {
  assert.equal(signerDigest(apk(null)), null, "v1 only or unsigned");
  assert.equal(signerDigest(apk([[0x42726577, Buffer.alloc(8)]])), null);
  assert.equal(signerDigest(apk([[0x7109871a, Buffer.from([9, 0, 0, 0, 1])]])), null, "truncated");
  assert.equal(signerDigest(Buffer.from("not a zip at all")), null);
  assert.equal(signerDigest(Buffer.alloc(0)), null);
});

// Self-signed certificates made with openssl: the subject of the Android debug key, and another.
const DEBUG_CERT = Buffer.from(
  "MIIBxDCCAWugAwIBAgIUWQ8Md6mWf5uxFXL36UTDj98+AFYwCgYIKoZIzj0EAwIwNzELMAkGA1UEBhMCVVMxEDAOBgNVBAoMB0FuZHJvaWQxFjAUBgNVBAMMDUFuZHJvaWQgRGVidWcwIBcNMjYxMDEyMTIwODIxWhgPMjA1NjEwMDQxMjA4MjFaMDcxCzAJBgNVBAYTAlVTMRAwDgYDVQQKDAdBbmRyb2lkMRYwFAYDVQQDDA1BbmRyb2lkIERlYnVnMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE5IHeTHGy3XSv1AYKRjODSIgdai6wQZizbLtzewqyloX0Pwn6p3OCftfH8FPDwZUtLZojYOiTvTK/2qQCNPkSaqNTMFEwHQYDVR0OBBYEFGX60MU+TpbzviD+m5Of82E5dDWlMB8GA1UdIwQYMBaAFGX60MU+TpbzviD+m5Of82E5dDWlMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDRwAwRAIgUgGA959tDH5q5ZD7zKjqI57FK9Rk8brhBvMWvlQtMmoCIC4j9ry5UhjYqejx5elnrvG86WpEQ6iLaNad62mOHKNd",
  "base64",
);
const RELEASE_CERT = Buffer.from(
  "MIIBujCCAWGgAwIBAgIUevMvTmpSVaOjAp0tRt4eVnYzUUgwCgYIKoZIzj0EAwIwMjELMAkGA1UEBhMCREUxEjAQBgNVBAoMCU1vdXNlV2VyazEPMA0GA1UEAwwGQXJjYWxvMCAXDTI2MTAxMjEyMDgyMVoYDzIwNTYxMDA0MTIwODIxWjAyMQswCQYDVQQGEwJERTESMBAGA1UECgwJTW91c2VXZXJrMQ8wDQYDVQQDDAZBcmNhbG8wWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASCUO3UBSw5LkKOoIx06s4+oIdkufysv1hqh2r67IaBB4ORXUZfTicqqIZ4rYcM8VLSStGTZBr588Jp+3ae+rVpo1MwUTAdBgNVHQ4EFgQUxR5lBDl5QiXENLlQfm0mNdEpWlkwHwYDVR0jBBgwFoAUxR5lBDl5QiXENLlQfm0mNdEpWlkwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNHADBEAiBilQgHXqO/kILqYzWmqtrR86crY+Kpz84/T+Z4LWXQggIgPc5llBkWVVfRFsX4c1CxTkaf1CVabcWKYUDz7jKh5Hk=",
  "base64",
);

test("a debug-signed APK is told apart by its certificate's subject", () => {
  const debug = signerSubject(apk([[0x7109871a, scheme([DEBUG_CERT])]]));
  assert.equal(debug, "C=US\nO=Android\nCN=Android Debug");
  assert.equal(isDebugSubject(debug), true);
  const release = signerSubject(apk([[0xf05368c0, scheme([RELEASE_CERT], true)]]));
  assert.equal(release, "C=DE\nO=MouseWerk\nCN=Arcalo");
  assert.equal(isDebugSubject(release), false);
  assert.equal(isDebugSubject("CN=Android Debugger"), false);
  // No certificate, or none X.509 can read: no subject (and not taken for a debug key).
  assert.equal(signerSubject(apk(null)), null);
  assert.equal(signerSubject(apk([[0x7109871a, scheme([certA])]])), null);
  assert.equal(isDebugSubject(null), false);
});

test("the command line prints the digest or fails", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "arcalo-apk-signer-"));
  try {
    const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "apk-signer.mjs");
    const signed = path.join(dir, "signed.apk");
    fs.writeFileSync(signed, apk([[0x7109871a, scheme([certA])]]));
    assert.equal(execFileSync(process.execPath, [script, signed], { encoding: "utf8" }), `${sha(certA)}\n`);
    const unsigned = path.join(dir, "unsigned.apk");
    fs.writeFileSync(unsigned, apk(null));
    assert.throws(() => execFileSync(process.execPath, [script, unsigned], { stdio: "pipe" }), /no APK Signing Block/);
    const debug = path.join(dir, "debug.apk");
    fs.writeFileSync(debug, apk([[0x7109871a, scheme([DEBUG_CERT])]]));
    assert.equal(execFileSync(process.execPath, [script, "--subject", debug], { encoding: "utf8" }), "C=US\nO=Android\nCN=Android Debug\n");
    assert.throws(() => execFileSync(process.execPath, [script, "--subject", signed], { stdio: "pipe" }), /no readable signing certificate/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
