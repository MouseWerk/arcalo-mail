// The SHA-256 of an APK's signing certificate, read from its APK Signing Block (scheme v3, else v2),
// as the app compares it (`signer` of android-latest.json, Android's apkContentsSigners): hex of
// the first signer's first certificate. Needs no Java or Android build tools (the release workflow
// lost the signer when build-tools 37 changed apksigner's output). The signature itself is not
// checked here; Android checks it on install, and the app compares the certificate.
//
//   node scripts/apk-signer.mjs <file.apk>              prints the digest (lowercase hex), exit 1 if none
//   node scripts/apk-signer.mjs --subject <file.apk>    prints the certificate's subject
//                                                       (`CN=Android Debug` for a debug key), exit 1 if none

import crypto from "node:crypto";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const MAGIC = "APK Sig Block 42";
const V2 = 0x7109871a;
const V3 = 0xf05368c0;

/** The id → value pairs of the APK Signing Block before the central directory, or null. */
export function signingBlock(apk) {
  // End of central directory: at least 22 bytes, a comment of up to 65535 after it.
  const from = Math.max(0, apk.length - 22 - 0xffff);
  let eocd = -1;
  for (let i = apk.length - 22; i >= from; i--) {
    if (apk.readUInt32LE(i) === 0x06054b50 && i + 22 + apk.readUInt16LE(i + 20) === apk.length) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) return null;
  const cd = apk.readUInt32LE(eocd + 16);
  if (cd < 32 || cd > apk.length || apk.toString("latin1", cd - 16, cd) !== MAGIC) return null;
  const size = Number(apk.readBigUInt64LE(cd - 24));
  const start = cd - size - 8;
  if (start < 0 || Number(apk.readBigUInt64LE(start)) !== size) return null;
  const pairs = new Map();
  for (let p = start + 8; p < cd - 24; ) {
    const len = Number(apk.readBigUInt64LE(p));
    if (len < 4 || p + 8 + len > cd - 24) return null;
    pairs.set(apk.readUInt32LE(p + 8), apk.subarray(p + 12, p + 8 + len));
    p += 8 + len;
  }
  return pairs;
}

/** A uint32-length-prefixed part of `buf` at `at`: [part, offset after it]. */
function prefixed(buf, at) {
  if (at + 4 > buf.length) throw new Error("truncated");
  const len = buf.readUInt32LE(at);
  if (at + 4 + len > buf.length) throw new Error("truncated");
  return [buf.subarray(at + 4, at + 4 + len), at + 4 + len];
}

/** The first certificate (DER) of the first signer of a v2 or v3 scheme block. */
export function firstCertificate(scheme) {
  const [signers] = prefixed(scheme, 0);
  const [signer] = prefixed(signers, 0);
  const [signedData] = prefixed(signer, 0);
  const [, afterDigests] = prefixed(signedData, 0);
  const [certificates] = prefixed(signedData, afterDigests);
  const [cert] = prefixed(certificates, 0);
  if (!cert.length) throw new Error("empty certificate");
  return cert;
}

/** The signing certificate (DER) of the v3 signer, else the v2 one; null without one. */
function signingCertificate(apk) {
  const pairs = signingBlock(apk);
  const scheme = pairs?.get(V3) ?? pairs?.get(V2);
  if (!scheme) return null;
  try {
    return firstCertificate(scheme);
  } catch {
    return null;
  }
}

/** SHA-256 of the signing certificate in lowercase hex, or null when the APK has no v2/v3 signature. */
export function signerDigest(apk) {
  const cert = signingCertificate(apk);
  return cert && crypto.createHash("sha256").update(cert).digest("hex");
}

/** The subject of the signing certificate (`C=US\nO=Android\nCN=Android Debug`), or null. */
export function signerSubject(apk) {
  const cert = signingCertificate(apk);
  if (!cert) return null;
  try {
    return new crypto.X509Certificate(cert).subject;
  } catch {
    return null;
  }
}

/** The subject of the key the Android build tools sign debug builds with. */
export const isDebugSubject = (subject) => /(^|\n)CN=Android Debug(\n|$)/.test(subject ?? "");

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const subject = process.argv[2] === "--subject";
  const file = process.argv[subject ? 3 : 2];
  if (!file) {
    console.error("usage: node scripts/apk-signer.mjs [--subject] <file.apk>");
    process.exit(2);
  }
  if (subject) {
    const text = signerSubject(fs.readFileSync(file));
    if (!text) {
      console.error(`${file}: no readable signing certificate`);
      process.exit(1);
    }
    console.log(text);
    process.exit(0);
  }
  const digest = signerDigest(fs.readFileSync(file));
  if (!digest) {
    console.error(`${file}: no APK Signing Block with a v2 or v3 signer`);
    process.exit(1);
  }
  console.log(digest);
}
