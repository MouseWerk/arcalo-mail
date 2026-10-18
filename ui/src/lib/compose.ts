// The compose sheet's state, and the drafts for new messages, replies and forwards: recipients,
// the „Re:“/„Fwd:“ subject, the reply headers, the quote (rich text or plain) and the signature.

import { t, type Lang } from "./i18n";
import { addressLine, displayName, fullDate } from "./format";
import type { Account, Address, Attachment, ComposeDraft, FileInfo, MessageView, Protect } from "./types";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";

export type ComposeState = {
  /** Which sheet this is: a new one for every message opened, so nothing of another message
   *  (the editor's text, a typed address) stays in it. */
  uid: number;
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
  /** Addresses typed into To, Cc and Bcc that are no chip yet; they count as recipients. */
  typing?: { to: string; cc: string; bcc: string };
  /** Signed and/or encrypted when sent (OpenPGP or S/MIME). */
  protect?: Protect | null;
  /** A reply to or forward of a decrypted message: the quote stays encrypted, or the sheet warns. */
  fromEncrypted?: Protect["method"] | null;
  /** An encrypted draft opened again: its files stay in the backend and go along when it is
   *  saved or sent (`names` for the sheet). */
  sealed?: { from: number; names: string[] } | null;
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

let lastUid = 0;
/** A new sheet's id. */
export const nextUid = () => ++lastUid;

/** The same message in a new sheet (put back after the undo of a send or a discard). */
export const reopened = (s: ComposeState): ComposeState => ({ ...s, uid: nextUid(), revision: s.revision + 1 });

const signatureText = (a: Account | null) => (a?.signature.trim() ? `-- \n${a.signature.trim()}` : "");

function base(account: Account | null, rich: boolean): ComposeState {
  return {
    uid: nextUid(),
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
  const p = m.protection;
  const decrypted = !!(p?.encrypted && p.decrypted);
  // Decrypted text from a block inside someone else's text is never quoted back (it could be a
  // captured message sent to get its content).
  const original = decrypted && p?.partial ? t("protect.quoteLeftOut", undefined, lang) : (m.text ?? m.summary.snippet).replace(/\s+$/, "");
  if (decrypted && p) {
    d.fromEncrypted = p.method;
    d.protect = { method: p.method, sign: false, encrypt: true };
  }
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

/** A stored message in the compose sheet: a scheduled one taken back, or a draft from Drafts
 *  (`draft_of`). `files`: its attachments as they are on disk now, `forwarded`: the stored
 *  attachments that go along. A message written as rich text opens as rich text. */
export function fromComposeDraft(d: ComposeDraft, files: FileInfo[] = [], forwarded: Attachment[] = []): ComposeState {
  const s = base(null, d.html != null);
  return {
    ...s,
    mode: "draft",
    accountId: d.account_id,
    to: d.to,
    cc: d.cc,
    bcc: d.bcc,
    showCc: d.cc.length > 0,
    showBcc: d.bcc.length > 0,
    subject: d.subject,
    html: d.html ?? "",
    text: d.html != null ? "" : d.text,
    attachments: files,
    forwarded,
    inReplyTo: d.in_reply_to,
    references: d.references,
    messageId: d.message_id,
    replyToId: d.reply_to_id,
    protect: d.protect ?? null,
    sealed: d.sealed_from != null ? { from: d.sealed_from, names: d.sealed_files ?? [] } : null,
  };
}

/** The plain text of editor HTML (for switching to plain text). */
export function editorHtmlToText(html: string): string {
  if (typeof document === "undefined") return html.replace(/<[^>]+>/g, "");
  // An inert document: nothing in it loads or runs (a detached element of the page would still
  // load its images).
  const div = new DOMParser().parseFromString(html.replace(/<br\s*\/?>/gi, "\n").replace(/<\/(p|div|li|h\d)>/gi, "$&\n"), "text/html").body;
  for (const q of Array.from(div.querySelectorAll("blockquote")))
    q.textContent = (q.textContent ?? "").replace(/\n+$/, "").split("\n").map((l) => `> ${l}`).join("\n") + "\n";
  return (div.textContent ?? "").replace(/\n{3,}/g, "\n\n").replace(/\s+$/, "");
}

/** The sheet with the addresses still being typed made recipients. */
export function committed(s: ComposeState): ComposeState {
  const t = s.typing;
  if (!t || !(t.to.trim() || t.cc.trim() || t.bcc.trim())) return s;
  return {
    ...s,
    to: [...s.to, ...splitAddresses(t.to)],
    cc: [...s.cc, ...splitAddresses(t.cc)],
    bcc: [...s.bcc, ...splitAddresses(t.bcc)],
    showCc: s.showCc || !!t.cc.trim(),
    showBcc: s.showBcc || !!t.bcc.trim(),
    typing: { to: "", cc: "", bcc: "" },
  };
}

/** Whether the sheet holds nothing worth keeping. */
export function isEmpty(state: ComposeState): boolean {
  const s = committed(state);
  const body = s.rich ? editorHtmlToText(s.html) : s.text;
  return !s.to.length && !s.cc.length && !s.bcc.length && !s.subject.trim() && !body.replace(/^-- [\s\S]*$/m, "").trim() && !s.attachments.length && !s.forwarded.length && !s.sealed?.names.length;
}

/** What the backend sends or saves. */
export function toDraft(state: ComposeState): ComposeDraft {
  const s = committed(state);
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
    protect: s.protect ?? null,
    sealed_from: s.sealed?.from ?? null,
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

/** Where the user's own text ends in the body: at the signature (`-- `), the quote of the
 *  message answered (its intro line, `>` lines, a blockquote) or the header of a forward. */
function ownEnd(lines: string[]): number {
  const i = lines.findIndex((l) => {
    const s = l.trim();
    return l === "-- " || s === "--" || s.startsWith("--------") || l.startsWith(">") || /(schrieb|wrote)[^:]*:$/.test(s);
  });
  return i < 0 ? lines.length : i;
}

/** The text the user wrote, without signature and quote (what the writing help works on). */
export function ownText(s: ComposeState): string {
  if (s.rich) {
    const { own } = splitHtml(s.html);
    return editorHtmlToText(own).trim();
  }
  const lines = s.text.replace(/\r\n/g, "\n").split("\n");
  return lines.slice(0, ownEnd(lines)).join("\n").trim();
}

/** The editor HTML split where the signature, the quote or a forward begins. */
function splitHtml(html: string): { own: string; rest: string } {
  const markers = [/<p>--\s?<\/p>/i, /<p>-- (?=<br|<\/p>)/i, /<blockquote/i, /<p>--------/i, /<p>[^<]*(schrieb|wrote)[^<]*:<\/p>/i];
  let at = html.length;
  for (const m of markers) {
    const hit = m.exec(html);
    if (hit && hit.index < at) at = hit.index;
  }
  // The intro line of a quote sits right before its blockquote.
  const before = html.slice(0, at);
  const intro = /<p>[^<]*<\/p>$/.exec(before);
  if (intro && /^<blockquote/i.test(html.slice(at)) && /(schrieb|wrote)/.test(intro[0])) at = intro.index;
  return { own: html.slice(0, at), rest: html.slice(at) };
}

/** The body with the user's own text replaced (signature and quote stay). */
export function replaceOwnText(s: ComposeState, text: string): ComposeState {
  if (s.rich) {
    const { rest } = splitHtml(s.html);
    return { ...s, html: `${textToEditorHtml(text.trim())}<p><br></p>${rest}`, revision: s.revision + 1 };
  }
  const lines = s.text.replace(/\r\n/g, "\n").split("\n");
  const rest = lines.slice(ownEnd(lines)).join("\n");
  return { ...s, text: rest ? `${text.trim()}\n\n${rest}` : text.trim(), revision: s.revision + 1 };
}
