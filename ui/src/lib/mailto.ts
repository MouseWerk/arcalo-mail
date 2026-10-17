// `mailto:` links inside a message, read as the core reads the system's e-mail links
// (crates/mail-core/src/mailto.rs): the addresses, `to`, `cc`, `bcc`, `subject` and `body`, and
// nothing else (a link must not choose the sender, attach a file or pose as a reply).

import type { Mailto } from "./types";

const MAX_LEN = 64 * 1024;

/** Percent-decoding that never throws: a broken escape stays as it is, `+` stays a plus. */
function decode(s: string): string {
  const bytes: number[] = [];
  const raw = new TextEncoder().encode(s);
  for (let i = 0; i < raw.length; i++) {
    if (raw[i] === 0x25 && i + 2 < raw.length) {
      const hex = String.fromCharCode(raw[i + 1], raw[i + 2]);
      if (/^[0-9a-fA-F]{2}$/.test(hex)) {
        bytes.push(parseInt(hex, 16));
        i += 2;
        continue;
      }
    }
    bytes.push(raw[i]);
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}

const oneLine = (s: string) => s.replace(/[\r\n]/g, " ").trim();
const addresses = (s: string) => s.split(",").map((a) => oneLine(a)).filter(Boolean);

/** The fields of a `mailto:` link; `null` for anything else. */
export function mailtoFromHref(href: string): Mailto | null {
  const raw = href.trim();
  if (raw.length > MAX_LEN || raw.slice(0, 7).toLowerCase() !== "mailto:") return null;
  const rest = raw.slice(7);
  const q = rest.indexOf("?");
  const [path, query] = q < 0 ? [rest, ""] : [rest.slice(0, q), rest.slice(q + 1)];
  const m: Mailto = { to: addresses(decode(path)), cc: [], bcc: [], subject: "", body: "" };
  for (const pair of query.split("&").filter(Boolean)) {
    const eq = pair.indexOf("=");
    const k = decode(eq < 0 ? pair : pair.slice(0, eq)).toLowerCase();
    const v = decode(eq < 0 ? "" : pair.slice(eq + 1));
    if (k === "to") m.to.push(...addresses(v));
    else if (k === "cc") m.cc.push(...addresses(v));
    else if (k === "bcc") m.bcc.push(...addresses(v));
    else if (k === "subject" && !m.subject) m.subject = oneLine(v);
    else if (k === "body" && !m.body) m.body = v.replace(/\r\n/g, "\n");
  }
  return m;
}
