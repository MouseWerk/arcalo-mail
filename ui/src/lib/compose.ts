// The compose sheet's state, and the drafts for new messages, replies and forwards: recipients,
// the „Re:“/„Fwd:“ subject, the reply headers, the quote (rich text or plain) and the signature.

import { t, type Lang } from "./i18n";
import { addressLine, displayName, fullDate } from "./format";
import type { Account, Address, Attachment, ComposeDraft, FileInfo, MessageView } from "./types";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";

export type ComposeState = {
  mode: ComposeMode;
  accountId: number | null;
  to: string[];
  cc: string[];
  bcc: string[];
  showCc: boolean;
  showBcc: boolean;
  subject: string;
  /** Rich text (the editor's HTML) or plain text. */
  rich: boolean;
  html: string;
  text: string;
  attachments: FileInfo[];
  /** Stored attachments that go along: a forwarded message's, or the edited draft's. */
  forwarded: Attachment[];
  inReplyTo: string | null;
  references: string[];
  /** The draft's Message-ID once it was saved, kept for the next save and for sending. */
  messageId: string | null;
  replyToId: number | null;
  /** Bumped when the body is replaced from outside, so the editor loads it again. */
  revision: number;
};

export const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** Plain text as editor HTML: paragraphs per line. */
export const textToEditorHtml = (s: string) =>
  s
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((l) => (l.trim() ? `<p>${escapeHtml(l)}</p>` : "<p><br></p>"))
    .join("");

/** The signature in the editor, with an empty line below it. */
const sigBlock = (sig: string) => (sig ? `${textToEditorHtml(sig)}<p><br></p>` : "");

const signatureText = (a: Account | null) => (a?.signature.trim() ? `-- \n${a.signature.trim()}` : "");

function base(account: Account | null, rich: boolean): ComposeState {
  return {
    mode: "new",
    accountId: account?.id ?? null,
    to: [],
    cc: [],
    bcc: [],
    showCc: false,
    showBcc: false,
    subject: "",
    rich,
    html: "",
    text: "",
    attachments: [],
    forwarded: [],
    inReplyTo: null,
    references: [],
    messageId: null,
    replyToId: null,
    revision: 0,
  };
}

/** A new message, with the account's signature below an empty first line. */
export function emptyDraft(account: Account | null, rich: boolean, to: string[] = []): ComposeState {
  const sig = signatureText(account);
  const d = base(account, rich);
  d.to = to;
  if (sig) {
    if (rich) d.html = `<p><br></p><p><br></p>${textToEditorHtml(sig)}`;
    else d.text = `\n\n${sig}`;
  }
  return d;
}

/** „Re: “ / „Fwd: “ once, whatever the sender's client wrote before (Re, RE, AW, Fw, WG). */
export function prefixed(prefix: "Re" | "Fwd", subject: string): string {
  const bare = subject.replace(/^\s*((re|aw|fw|fwd|wg)(\[\d+\])?\s*:\s*)+/i, "");
  return `${prefix}: ${bare}`;
}

export const quoteText = (text: string) =>
  text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((l) => (l.startsWith(">") ? `>${l}` : `> ${l}`))
    .join("\n");

const sameAddress = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();
const line = (a: Address) => addressLine([a]);

/** A reply, reply to all or forward of a message. */
export function replyDraft(m: MessageView, mode: "reply" | "replyAll" | "forward", account: Account | null, lang: Lang, rich: boolean): ComposeState {
  const d = base(account, rich);
  d.mode = mode;
  const from = m.summary.from;
  const original = (m.text ?? m.summary.snippet).replace(/\s+$/, "");
  const sig = signatureText(account);
  if (mode === "forward") {
    d.subject = prefixed("Fwd", m.summary.subject);
    d.forwarded = m.files;
    const head = [
      `-------- ${t("compose.forwardHeader", undefined, lang)} --------`,
      `${t("read.from", undefined, lang)}: ${from ? line(from) : ""}`,
      `${t("read.date", undefined, lang)}: ${fullDate(m.summary.date, lang)}`,
      `${t("read.to", undefined, lang)}: ${addressLine(m.to)}`,
      `${t("compose.subject", undefined, lang)}: ${m.summary.subject}`,
    ];
    if (rich) d.html = `<p><br></p><p><br></p>${sigBlock(sig)}${textToEditorHtml(head.join("\n"))}<p><br></p>${textToEditorHtml(original)}`;
    else d.text = `\n\n${sig ? `${sig}\n\n` : ""}${head.join("\n")}\n\n${original}`;
    return d;
  }
  const own = account?.email ?? "";
  const replyTo: Address[] = m.reply_to.length ? m.reply_to : from ? [from] : [];
  d.to = replyTo.filter((a) => !sameAddress(a.email, own) || replyTo.length === 1).map(line);
  if (mode === "replyAll") {
    d.cc = [...m.to, ...m.cc]
      .filter((a) => !sameAddress(a.email, own) && !replyTo.some((x) => sameAddress(x.email, a.email)))
      .filter((a, i, all) => all.findIndex((b) => sameAddress(a.email, b.email)) === i)
      .map(line);
    d.showCc = d.cc.length > 0;
  }
  d.subject = prefixed("Re", m.summary.subject);
  d.inReplyTo = m.message_id;
  d.references = [...m.references, ...(m.message_id ? [m.message_id] : [])];
  d.replyToId = m.summary.id;
  const intro = t("compose.quoteHeader", { date: fullDate(m.summary.date, lang), name: displayName(from, lang) }, lang);
  if (rich) {
    d.html = `<p><br></p><p><br></p>${sigBlock(sig)}<p>${escapeHtml(intro)}</p><blockquote type="cite">${textToEditorHtml(original)}</blockquote>`;
  } else {
    d.text = `\n\n${sig ? `${sig}\n\n` : ""}${intro}\n${quoteText(original)}`;
  }
  return d;
}

/** A draft from the Drafts folder, opened to go on writing. */
export function draftFromMessage(m: MessageView, account: Account | null, rich: boolean): ComposeState {
  const d = base(account, rich);
  d.mode = "draft";
  d.to = m.to.map(line);
  d.cc = m.cc.map(line);
  d.showCc = d.cc.length > 0;
  d.subject = m.summary.subject;
  d.messageId = m.message_id;
  d.inReplyTo = m.in_reply_to;
  d.references = m.references;
  d.forwarded = m.files;
  const text = m.text ?? "";
  if (rich) d.html = textToEditorHtml(text);
  else d.text = text;
  return d;
}

/** The plain text of editor HTML (for switching to plain text). */
export function editorHtmlToText(html: string): string {
  if (typeof document === "undefined") return html.replace(/<[^>]+>/g, "");
  const div = document.createElement("div");
  div.innerHTML = html.replace(/<br\s*\/?>/gi, "\n").replace(/<\/(p|div|li|h\d)>/gi, "$&\n");
  for (const q of Array.from(div.querySelectorAll("blockquote")))
    q.textContent = (q.textContent ?? "").replace(/\n+$/, "").split("\n").map((l) => `> ${l}`).join("\n") + "\n";
  return (div.textContent ?? "").replace(/\n{3,}/g, "\n\n").replace(/\s+$/, "");
}

/** Whether the sheet holds nothing worth keeping. */
export function isEmpty(s: ComposeState): boolean {
  const body = s.rich ? editorHtmlToText(s.html) : s.text;
  return !s.to.length && !s.cc.length && !s.bcc.length && !s.subject.trim() && !body.replace(/^-- [\s\S]*$/m, "").trim() && !s.attachments.length && !s.forwarded.length;
}

/** What the backend sends or saves. */
export function toDraft(s: ComposeState): ComposeDraft {
  return {
    account_id: s.accountId ?? 0,
    to: s.to,
    cc: s.cc,
    bcc: s.bcc,
    subject: s.subject,
    text: s.rich ? "" : s.text,
    html: s.rich ? s.html : null,
    attachments: s.attachments.map((a) => a.path),
    in_reply_to: s.inReplyTo,
    references: s.references,
    message_id: s.messageId,
    reply_to_id: s.replyToId,
    forwarded: s.forwarded.map((a) => a.id),
  };
}

/** An address as typed: `Name <a@b.org>` or `a@b.org`. */
export function isValidAddress(s: string): boolean {
  const m = s.trim().match(/<([^<>]+)>$/);
  const email = (m ? m[1] : s).trim();
  return /^[^\s@<>()",;]+@[^\s@<>()",;]+\.[^\s@<>()",;]{2,}$/.test(email);
}

/** Splits typed or pasted addresses at commas and semicolons (names in quotes may contain commas). */
export function splitAddresses(field: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quoted = false;
  for (const c of field) {
    if (c === '"') quoted = !quoted;
    if (!quoted && (c === "," || c === ";" || c === "\n")) {
      if (cur.trim()) out.push(cur.trim());
      cur = "";
    } else cur += c;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}
