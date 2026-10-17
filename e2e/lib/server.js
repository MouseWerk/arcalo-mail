// The IMAP and SMTP test server on this computer (crates/mail-testserver), started per test
// file: its ports, and the control port to deliver mail, read the state and what was sent,
// make commands fail and take the server offline.

import { spawn } from "node:child_process";
import path from "node:path";
import readline from "node:readline";

const ROOT = path.resolve(import.meta.dirname, "../..");
export const SERVER_BIN = path.resolve(process.env.ARCALO_MAIL_TESTSERVER ?? path.join(ROOT, "target/debug/arcalo-mail-testserver"));

export const USER = "maria@example.org";
export const PASSWORD = "geheim";

export async function startServer({ user = USER, password = PASSWORD, args = [] } = {}) {
  const child = spawn(SERVER_BIN, ["--user", user, "--password", password, ...args], { stdio: ["ignore", "pipe", "inherit"] });
  process.on("exit", () => child.kill());
  const line = await new Promise((resolve, reject) => {
    const rl = readline.createInterface({ input: child.stdout });
    rl.once("line", resolve);
    child.once("exit", (code) => reject(new Error(`test server exited with ${code}`)));
  });
  const ports = JSON.parse(line);
  const base = `http://127.0.0.1:${ports.control}`;
  const post = async (p, body) => {
    const r = await fetch(`${base}${p}`, { method: "POST", body });
    if (!r.ok) throw new Error(`${p}: ${r.status} ${await r.text()}`);
    return r.headers.get("content-type")?.includes("json") ? r.json() : r.text();
  };
  return {
    ...ports,
    user,
    password,
    /** Delivers a message (RFC 5322 text); `opts`: mailbox, seen, flagged, answered, date. */
    deliver: (raw, opts = {}) => {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(opts)) q.set(k, typeof v === "boolean" ? (v ? "1" : "0") : String(v));
      return post(`/deliver?${q}`, raw);
    },
    state: async () => (await fetch(`${base}/state`)).json(),
    /** Requests to the senders' web side (`/unsubscribe…`, `/track…`, `/img…` on the control port). */
    web: async () => (await fetch(`${base}/web`)).json(),
    /** An address of the senders' web side on this server. */
    webUrl: (p) => `${base}${p}`,
    sent: async () => (await fetch(`${base}/sent`)).json(),
    sentRaw: async (i) => (await fetch(`${base}/sent/raw?i=${i}`)).text(),
    offline: () => post("/offline"),
    online: () => post("/online"),
    fail: (cmd) => post(`/fail?cmd=${encodeURIComponent(cmd)}`),
    setFlags: (mailbox, uid, flags) => post(`/flags?${new URLSearchParams({ mailbox, uid: String(uid), ...Object.fromEntries(Object.entries(flags).map(([k, v]) => [k, v ? "1" : "0"])) })}`),
    folder: (name) => post(`/folder?name=${encodeURIComponent(name)}`),
    delay: (ms) => post(`/delay?ms=${ms}`),
    setPassword: (value) => post(`/password?value=${encodeURIComponent(value)}`),
    /** The identity provider refuses the next sign-in (`admin_consent`, `access_denied`). */
    oauthFail: (mode) => post(`/oauth/fail?mode=${encodeURIComponent(mode)}`),
    /** SMTP refuses XOAUTH2 with 535 5.7.139 (SMTP AUTH off for the mailbox). */
    smtpAuth: (enabled) => post(`/smtp-auth?disabled=${enabled ? "0" : "1"}`),
    /** What the app needs to sign in with Microsoft against this server. */
    microsoftEnv: () => ({
      ARCALO_MAIL_MS_AUTHORITY: `http://127.0.0.1:${ports.control}/oauth`,
      ARCALO_MAIL_MS_CLIENT_ID: "e2e-test-client",
      ARCALO_MAIL_MS_IMAP: `127.0.0.1:${ports.imap}`,
      ARCALO_MAIL_MS_SMTP: `127.0.0.1:${ports.smtp}`,
    }),
    /** What the setup needs to find this server, JMAP included. */
    discoveryEnv: () => ({
      ARCALO_MAIL_AUTOCONFIG: `http://127.0.0.1:${ports.control}/autoconfig?emailaddress={email}`,
      ARCALO_MAIL_JMAP_DISCOVERY: `http://127.0.0.1:${ports.control}/.well-known/jmap`,
    }),
    /** The transport of an account on this server (plain text: loopback only). */
    transport: () => ({
      kind: "imap",
      imap: { host: "127.0.0.1", port: ports.imap, security: "plain" },
      smtp: { host: "127.0.0.1", port: ports.smtp, security: "plain" },
      username: user,
      auth: { kind: "password" },
    }),
    stop: () => child.kill(),
  };
}

/** A plain message for tests. */
export function message({ from = "Jonas Berger <jonas@example.org>", to = "Maria Muster <maria@example.org>", cc, subject, body, id, date = new Date(), inReplyTo, html, attachments = [], headers = [] }) {
  const mid = id ?? `${Math.random().toString(36).slice(2)}@example.org`;
  const head = [
    `From: ${from}`,
    `To: ${to}`,
    ...(cc ? [`Cc: ${cc}`] : []),
    `Subject: ${subject}`,
    `Date: ${date.toUTCString().replace("GMT", "+0000")}`,
    `Message-ID: <${mid}>`,
    ...(inReplyTo ? [`In-Reply-To: <${inReplyTo}>`, `References: <${inReplyTo}>`] : []),
    ...headers,
    "MIME-Version: 1.0",
  ];
  const text = body.replace(/\n/g, "\r\n");
  if (!html && !attachments.length) return [...head, "Content-Type: text/plain; charset=utf-8", "Content-Transfer-Encoding: 8bit", "", text, ""].join("\r\n");
  const alt = html
    ? ["Content-Type: multipart/alternative; boundary=\"ALT\"", "", "--ALT", "Content-Type: text/plain; charset=utf-8", "", text, "--ALT", "Content-Type: text/html; charset=utf-8", "", html, "--ALT--", ""].join("\r\n")
    : ["Content-Type: text/plain; charset=utf-8", "", text, ""].join("\r\n");
  if (!attachments.length) return [...head, alt].join("\r\n");
  const parts = attachments.map((a) =>
    [
      "--MIX",
      `Content-Type: ${a.type}; name="${a.name}"`,
      `Content-Disposition: attachment; filename="${a.name}"`,
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from(a.data).toString("base64").replace(/.{76}/g, "$&\r\n"),
    ].join("\r\n"),
  );
  return [...head, 'Content-Type: multipart/mixed; boundary="MIX"', "", "--MIX", alt, ...parts, "--MIX--", ""].join("\r\n");
}

/** Adds the server's account to the app without the setup screen. */
export async function addAccount(app, server, { name = "", sender = "Maria Muster" } = {}) {
  return app.invoke("add_account", {
    draft: { provider: "imap", name, email: server.user, sender_name: sender, transport: server.transport() },
    password: server.password,
  });
}
