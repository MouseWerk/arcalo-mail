// A local update feed for the update tests, as Arcalo's e2e/lib/update-feed.js: a throwaway
// minisign key (made per run, never the release key), `latest.json` and one update file, with
// switches for what a user can meet (a tampered file, a download that trickles in).

import crypto from "node:crypto";
import http from "node:http";

/** A fresh Ed25519 key in minisign format, as the app reads it. */
export function throwawayKey() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
  const id = crypto.randomBytes(8);
  const pub = `untrusted comment: minisign public key (arcalo mail e2e)\n${Buffer.concat([Buffer.from("Ed"), id, raw]).toString("base64")}\n`;
  return {
    /** `ARCALO_MAIL_UPDATE_PUBKEY`: base64 of the public key file (as plugins.updater.pubkey). */
    pubkey: Buffer.from(pub).toString("base64"),
    /** The `.sig` content for `data`: base64 of a minisign signature file naming `version`. */
    sign(data, version) {
      const sig = crypto.sign(null, data, privateKey);
      const trusted = `timestamp:${Math.floor(Date.now() / 1000)}\tfile:arcalo-mail-update.bin\tversion:${version}`;
      const global = crypto.sign(null, Buffer.concat([sig, Buffer.from(trusted)]), privateKey);
      const file = [
        "untrusted comment: signature from arcalo mail e2e key",
        Buffer.concat([Buffer.from("Ed"), id, sig]).toString("base64"),
        `trusted comment: ${trusted}`,
        global.toString("base64"),
        "",
      ].join("\n");
      return Buffer.from(file).toString("base64");
    },
  };
}

/**
 * Serves `/latest.json` and `/update.bin` for `linux-x86_64`. `feed.mode`: "ok", "tampered" (one
 * byte changed after signing) or "throttle" (the file trickles in over a few seconds). Range
 * requests get the rest of the file (206); `feed.ranges` lists the offsets asked for.
 */
export async function startFeed({ key, version, size = 2 * 1024 * 1024, notes }) {
  const file = crypto.randomBytes(size);
  const feed = { mode: "ok", version, ranges: [], requests: [] };
  const server = http.createServer((req, res) => {
    feed.requests.push(req.url);
    const base = `http://127.0.0.1:${server.address().port}`;
    if (req.url.startsWith("/latest.json")) {
      const body = {
        version: feed.version,
        notes,
        pub_date: "2026-11-02T12:00:00Z",
        platforms: { "linux-x86_64": { signature: key.sign(file, feed.version), url: `${base}/update.bin` } },
      };
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify(body));
    }
    if (req.url === "/update.bin") {
      let bytes = Buffer.from(file);
      if (feed.mode === "tampered") bytes[bytes.length >> 1] ^= 0xff;
      const range = /^bytes=(\d+)-$/.exec(req.headers.range ?? "");
      if (range) {
        const from = Number(range[1]);
        feed.ranges.push(from);
        bytes = bytes.subarray(Math.min(from, bytes.length));
        res.writeHead(206, { "content-type": "application/octet-stream", "content-length": bytes.length, "content-range": `bytes ${from}-${file.length - 1}/${file.length}` });
      } else res.writeHead(200, { "content-type": "application/octet-stream", "content-length": bytes.length });
      if (feed.mode === "throttle") {
        let at = 0;
        const step = Math.ceil(file.length / 40);
        const tick = setInterval(() => {
          if (res.destroyed) return clearInterval(tick);
          res.write(bytes.subarray(at, at + step));
          at += step;
          if (at >= bytes.length) {
            clearInterval(tick);
            res.end();
          }
        }, 200);
        return;
      }
      return res.end(bytes);
    }
    res.writeHead(404).end();
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  feed.url = `http://127.0.0.1:${server.address().port}/latest.json`;
  feed.close = () =>
    new Promise((r) => {
      server.close(r);
      server.closeAllConnections();
    });
  return feed;
}
