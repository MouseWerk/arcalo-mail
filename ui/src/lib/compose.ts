// Prefilled drafts for reply, reply all and forward.

import { t, type Lang } from "./i18n";
import { addressLine, displayName, fullDate } from "./format";
import type { Account, Address, MessageView } from "./types";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";

export type ComposeState = { mode: ComposeMode; accountId: number | null; to: string; cc: string; subject: string; body: string };

export const emptyDraft = (accountId: number | null): ComposeState => ({ mode: "new", accountId, to: "", cc: "", subject: "", body: "" });

/** „Re: “ / „Fwd: “ once, whatever the sender's client wrote before (Re, RE, AW, Fw, WG). */
export function prefixed(prefix: "Re" | "Fwd", subject: string): string {
  const bare = subject.replace(/^\s*((re|aw|fw|fwd|wg)(\[\d+\])?\s*:\s*)+/i, "");
  return `${prefix}: ${bare}`;
}

const quote = (text: string) =>
  text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((l) => (l.startsWith(">") ? `>${l}` : `> ${l}`))
    .join("\n");

const sameAddress = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

export function replyDraft(m: MessageView, mode: Exclude<ComposeMode, "new">, account: Account | null, lang: Lang): ComposeState {
  const from = m.summary.from;
  const text = m.text ?? m.summary.snippet;
  const own = account?.email ?? "";
  if (mode === "forward") {
    const header = [
      `-------- ${t("compose.forwardHeader", undefined, lang)} --------`,
      `${t("read.from", undefined, lang)}: ${from ? addressLine([from]) : ""}`,
      `${t("read.to", undefined, lang)}: ${addressLine(m.to)}`,
      `${t("compose.subject", undefined, lang)}: ${m.summary.subject}`,
    ].join("\n");
    return { mode, accountId: account?.id ?? null, to: "", cc: "", subject: prefixed("Fwd", m.summary.subject), body: `\n\n${header}\n\n${text}` };
  }
  const to: Address[] = from ? [from] : [];
  const cc: Address[] =
    mode === "replyAll"
      ? [...m.to, ...m.cc].filter((a) => !sameAddress(a.email, own) && !to.some((x) => sameAddress(x.email, a.email)))
      : [];
  const intro = t("compose.quoteHeader", { date: fullDate(m.summary.date, lang), name: displayName(from, lang) }, lang);
  return {
    mode,
    accountId: account?.id ?? null,
    to: addressLine(to),
    cc: addressLine(cc),
    subject: prefixed("Re", m.summary.subject),
    body: `\n\n${intro}\n${quote(text)}`,
  };
}

/** Splits an address field at commas and semicolons (names in quotes may contain commas). */
export function splitAddresses(field: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quoted = false;
  for (const c of field) {
    if (c === '"') quoted = !quoted;
    if (!quoted && (c === "," || c === ";")) {
      if (cur.trim()) out.push(cur.trim());
      cur = "";
    } else cur += c;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}
