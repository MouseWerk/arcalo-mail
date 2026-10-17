import {
  AlarmClock,
  ArrowLeft,
  Bold,
  Check,
  ChevronDown,
  File,
  FileText,
  Sparkles,
  Square,
  TriangleAlert,
  Italic,
  Link,
  List,
  Lock,
  MoreHorizontal,
  ShieldCheck,
  ListOrdered,
  Paperclip,
  Quote,
  RemoveFormatting,
  Send,
  Trash2,
  Type,
  Underline,
  X,
} from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { editorHtmlToText, isValidAddress, ownText, replaceOwnText, splitAddresses, textToEditorHtml, type ComposeState } from "../lib/compose";
import { api, onFileDrop, pickFiles } from "../lib/api";
import { fileSize } from "../lib/format";
import { useAi } from "../lib/ai";
import { matchSnippets, snippetQuery, today } from "../lib/snippets";
import type { Abilities, Account, Contact, Destination, ProtectMethod, Template, WritingStyle } from "../lib/types";
import { Menu, PromptDialog, Spinner, trapFocus, type MenuItem } from "./ui";
import { followupItems, sendLaterItems } from "./TimeMenus";
import { AiText, PrivacyLine } from "./Assistant";

type Props = {
  draft: ComposeState;
  accounts: Account[];
  /** Why sending or saving failed (the sheet stays open with the text). */
  error: { title: string; detail: string } | null;
  sending: boolean;
  /** The draft as it is now is saved („Gespeichert“). */
  saved?: boolean;
  /** Full screen (a phone): Tab stays in the sheet. Docked on a wide window it does not. */
  modal?: boolean;
  /** A phone: an app bar at the top (back, attach, send, more) instead of the footer, the
   *  formatting bar over the keyboard only while writing. */
  phone?: boolean;
  onChange: (d: ComposeState) => void;
  /** Sends; `followupSecs`: „Erinnern, wenn keine Antwort“ that long after. */
  onSend: (followupSecs: number | null) => void;
  /** „Später senden“ at `at` (Unix seconds), or `null` to pick the time. */
  onSendLater: (at: number | null, followupSecs: number | null) => void;
  onSaveDraft: () => void;
  onClose: () => void;
  onDiscard: () => void;
  /** The AI assistant is set up (the writing help). */
  aiOn?: boolean;
  /** New messages are signed with this method when the key is there (Settings → Verschlüsselung). */
  signByDefault?: ProtectMethod | null;
};

/**
 * Signing and encrypting: what the account's keys and the recipients' keys allow, asked again
 * when the recipients change; signing by default once it is known to be possible.
 */
function useAbilities(draft: ComposeState, onChange: (d: ComposeState) => void, signByDefault: ProtectMethod | null | undefined) {
  const [abilities, setAbilities] = useState<Abilities | null>(null);
  const recipients = [...draft.to, ...draft.cc, ...draft.bcc];
  const key = `${draft.accountId}|${recipients.join(",")}`;
  const defaulted = useRef(false);
  const latest = useRef(draft);
  latest.current = draft;
  useEffect(() => {
    if (draft.accountId == null) return;
    let gone = false;
    const timer = window.setTimeout(() => {
      void api
        .cryptoAbilities(draft.accountId!, recipients)
        .then((a) => {
          if (gone) return;
          setAbilities(a);
          const d = latest.current;
          if (!defaulted.current && d.protect === undefined && a.pgp_prefer_encrypt) {
            // Every recipient asked for encrypted mail (Autocrypt), and the sender can.
            defaulted.current = true;
            onChange({ ...d, protect: { method: "pgp", sign: signByDefault === "pgp", encrypt: true } });
          } else if (!defaulted.current && d.protect === undefined && signByDefault && (signByDefault === "pgp" ? a.pgp_sign : a.smime_sign)) {
            defaulted.current = true;
            onChange({ ...d, protect: { method: signByDefault, sign: true, encrypt: false } });
          }
        })
        .catch(() => {});
    }, 300);
    return () => {
      gone = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return abilities;
}

/** The snippet chooser: what was typed after `;;`, where the caret is. */
type SnippetPick = { query: string; list: Template[]; active: number };

const STYLE: Record<WritingStyle, TKey> = {
  shorter: "compose.helpShorter",
  friendlier: "compose.helpFriendlier",
  formal: "compose.helpFormal",
  grammar: "compose.helpGrammar",
};

const TITLE: Record<ComposeState["mode"], TKey> = {
  new: "compose.title",
  reply: "compose.reply",
  replyAll: "compose.replyAll",
  forward: "compose.forward",
  draft: "compose.draft",
};

/** To, Cc or Bcc: addresses as chips, suggestions from the address book while typing. What is
 *  typed and not yet a chip lives in the sheet's state (`text`), so it goes out with the message
 *  even when it is sent before the field is left. */
function AddressField({
  label,
  values,
  text,
  onChange,
  onText,
  autoFocus,
  extra,
}: {
  label: string;
  values: string[];
  text: string;
  onChange: (v: string[]) => void;
  onText: (v: string) => void;
  autoFocus?: boolean;
  extra?: React.ReactNode;
}) {
  const t = useT();
  const [hits, setHits] = useState<Contact[]>([]);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const id = useRef(`addr-${Math.random().toString(36).slice(2)}`).current;
  // The latest values for the blur timer, which runs after a render or two.
  const latest = useRef({ values, text, onChange, onText });
  latest.current = { values, text, onChange, onText };
  const blurTimer = useRef<number | null>(null);

  useEffect(() => {
    if (autoFocus) input.current?.focus();
    return () => {
      if (blurTimer.current != null) clearTimeout(blurTimer.current);
    };
  }, []);

  useEffect(() => {
    const q = text.trim();
    if (!q) {
      setHits([]);
      return;
    }
    let alive = true;
    const timer = setTimeout(() => {
      void api.suggest(q).then((list) => {
        if (!alive) return;
        setHits(list.filter((c) => !latest.current.values.some((v) => v.toLowerCase().includes(c.email.toLowerCase()))));
        setActive(0);
      });
    }, 100);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [text]);

  const commit = (raw: string) => {
    const parts = splitAddresses(raw);
    const { values, onChange, onText } = latest.current;
    if (parts.length) onChange([...values, ...parts]);
    onText("");
    setHits([]);
  };
  const pick = (c: Contact) => commit(c.name ? `${/[,;"]/.test(c.name) ? `"${c.name.replace(/"/g, "")}"` : c.name} <${c.email}>` : c.email);

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    // Ctrl+Enter sends: the sheet takes the typed address along.
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) return;
    if (hits.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setActive((a) => (a + (e.key === "ArrowDown" ? 1 : -1) + hits.length) % hits.length);
    } else if ((e.key === "Enter" || e.key === "Tab") && hits.length && text.trim()) {
      e.preventDefault();
      pick(hits[active]);
    } else if ((e.key === "Enter" || e.key === "," || e.key === ";") && text.trim()) {
      e.preventDefault();
      commit(text);
    } else if (e.key === "Backspace" && !text && values.length) {
      onChange(values.slice(0, -1));
    } else if (e.key === "Escape" && hits.length) {
      e.stopPropagation();
      setHits([]);
    }
  };

  return (
    <div className="field address-field">
      <label htmlFor={id}>{label}</label>
      <div className="chips" onMouseDown={(e) => e.target === e.currentTarget && (e.preventDefault(), input.current?.focus())}>
        {values.map((v, i) => (
          <span key={`${v}-${i}`} className={`chip${isValidAddress(v) ? "" : " chip-invalid"}`} title={isValidAddress(v) ? v : t("compose.invalidAddress")}>
            <bdi className="chip-text">{v.replace(/\s*<[^>]+>$/, "") || v}</bdi>
            <button type="button" className="chip-remove" aria-label={t("compose.removeRecipient", { name: v })} onClick={() => onChange(values.filter((_, j) => j !== i))}>
              <X size={12} strokeWidth={2} aria-hidden />
            </button>
          </span>
        ))}
        <input
          ref={input}
          id={id}
          className="chip-input"
          dir="auto"
          value={text}
          role="combobox"
          aria-expanded={hits.length > 0}
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={hits.length ? `${id}-${active}` : undefined}
          onChange={(e) => {
            const v = e.target.value;
            if (/[,;]/.test(v)) commit(v);
            else onText(v);
          }}
          onKeyDown={onKey}
          onBlur={() => {
            if (blurTimer.current != null) clearTimeout(blurTimer.current);
            blurTimer.current = window.setTimeout(() => {
              blurTimer.current = null;
              if (latest.current.text.trim()) commit(latest.current.text);
              setHits([]);
            }, 120);
          }}
          onPaste={(e) => {
            const pasted = e.clipboardData.getData("text");
            if (/[,;\n]/.test(pasted)) {
              e.preventDefault();
              commit(text + pasted);
            }
          }}
        />
        {hits.length > 0 && (
          <ul className="suggestions" role="listbox" id={`${id}-list`}>
            {hits.map((c, i) => (
              <li
                key={c.email}
                id={`${id}-${i}`}
                role="option"
                aria-selected={i === active}
                className="suggestion"
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(c);
                }}
              >
                <bdi className="suggestion-name">{c.name || c.email}</bdi>
                {c.name && <span className="suggestion-email">{c.email}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
      {extra}
    </div>
  );
}

/** The rich text editor: a content-editable area with a few formats. `onTyped` gets the text
 *  before the caret after each input (snippets); `onKey` sees the keys first. */
function Editor({
  html,
  revision,
  onChange,
  label,
  onTyped,
  onKey,
}: {
  html: string;
  revision: number;
  onChange: (h: string) => void;
  label: string;
  onTyped?: (before: string) => void;
  onKey?: (e: KeyboardEvent<HTMLElement>) => boolean;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [linking, setLinking] = useState(false);
  const saved = useRef<Range | null>(null);
  useEffect(() => {
    if (ref.current && ref.current.innerHTML !== html) ref.current.innerHTML = html || "<p><br></p>";
  }, [revision]);
  const startLink = () => {
    const sel = window.getSelection();
    saved.current = sel && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
    setLinking(true);
  };
  const run = (cmd: string, value?: string) => {
    ref.current?.focus();
    document.execCommand(cmd, false, value);
    if (ref.current) onChange(ref.current.innerHTML);
  };
  const tool = (label: string, icon: React.ReactNode, cmd: () => void) => (
    <button type="button" className="icon-btn icon-btn-sm" aria-label={label} title={label} onMouseDown={(e) => e.preventDefault()} onClick={cmd}>
      {icon}
    </button>
  );
  return (
    <>
      <div className="format-bar" role="toolbar" aria-label={t("compose.format")}>
        {tool(t("compose.bold"), <Bold size={15} strokeWidth={2} aria-hidden />, () => run("bold"))}
        {tool(t("compose.italic"), <Italic size={15} strokeWidth={2} aria-hidden />, () => run("italic"))}
        {tool(t("compose.underline"), <Underline size={15} strokeWidth={2} aria-hidden />, () => run("underline"))}
        <span className="format-sep" />
        {tool(t("compose.bullets"), <List size={15} strokeWidth={2} aria-hidden />, () => run("insertUnorderedList"))}
        {tool(t("compose.numbers"), <ListOrdered size={15} strokeWidth={2} aria-hidden />, () => run("insertOrderedList"))}
        {tool(t("compose.quote"), <Quote size={15} strokeWidth={2} aria-hidden />, () => run("formatBlock", "blockquote"))}
        {tool(t("compose.link"), <Link size={15} strokeWidth={2} aria-hidden />, startLink)}
        {tool(t("compose.clearFormat"), <RemoveFormatting size={15} strokeWidth={2} aria-hidden />, () => run("removeFormat"))}
      </div>
      <div
        ref={ref}
        className="editor"
        dir="auto"
        contentEditable
        role="textbox"
        aria-multiline="true"
        aria-label={label}
        suppressContentEditableWarning
        onInput={() => {
          if (!ref.current) return;
          onChange(ref.current.innerHTML);
          const sel = window.getSelection();
          const node = sel?.anchorNode;
          if (onTyped && node && node.nodeType === Node.TEXT_NODE) onTyped((node.textContent ?? "").slice(0, sel.anchorOffset));
          else onTyped?.("");
        }}
        onKeyDown={(e) => {
          // Ctrl/Cmd+K is „Link einfügen“ here, as in other editors (not the command palette).
          if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "k") {
            e.preventDefault();
            e.stopPropagation();
            startLink();
            return;
          }
          if (onKey?.(e)) e.preventDefault();
        }}
        onPaste={(e) => {
          // Pasted text comes as text; formatting from other programs stays out.
          const text = e.clipboardData.getData("text/plain");
          if (text && !e.clipboardData.types.includes("text/html")) return;
          e.preventDefault();
          document.execCommand("insertHTML", false, textToEditorHtml(text));
        }}
      />
      {linking && (
        <PromptDialog
          title={t("compose.linkTitle")}
          label={t("compose.linkAddress")}
          initial="https://"
          confirm={t("compose.linkInsert")}
          onDone={(url) => {
            const sel = window.getSelection();
            ref.current?.focus();
            if (saved.current && sel) {
              sel.removeAllRanges();
              sel.addRange(saved.current);
            }
            if (/^(https?:|mailto:)/i.test(url)) {
              if (sel && sel.isCollapsed) document.execCommand("insertHTML", false, `<a href="${url.replace(/"/g, "%22")}">${url.replace(/</g, "&lt;")}</a>`);
              else run("createLink", url);
            }
            if (ref.current) onChange(ref.current.innerHTML);
          }}
          onClose={() => setLinking(false)}
        />
      )}
    </>
  );
}

/** The compose sheet: bottom right on a wide window, full screen on a phone. Esc closes it (and
 *  keeps the draft), Ctrl+Enter sends. Files dropped on the window are attached. */
export function ComposeSheet({ draft, accounts, error, sending, saved, modal, phone, onChange, onSend, onSendLater, onSaveDraft, onClose, onDiscard, aiOn, signByDefault }: Props) {
  const t = useT();
  const lang = currentLang();
  const abilities = useAbilities(draft, onChange, signByDefault);
  const protect = draft.protect ?? null;
  /** The protection menu: per method signing and encrypting, as the keys allow. */
  const protectItems = (): MenuItem[] => {
    const a = abilities;
    const items: MenuItem[] = [];
    const methods: [ProtectMethod, string, boolean, boolean, string[]][] = [
      ["pgp", "OpenPGP", !!a?.pgp_sign, !!a?.pgp_encrypt, a?.pgp_missing ?? []],
      ["smime", "S/MIME", !!a?.smime_sign, !!a?.smime_encrypt, a?.smime_missing ?? []],
    ];
    for (const [method, name, canSign, canEncrypt, missing] of methods) {
      const on = protect?.method === method ? protect : null;
      const set = (sign: boolean, encrypt: boolean) => onChange({ ...draft, protect: sign || encrypt ? { method, sign, encrypt } : null });
      items.push({ heading: name });
      items.push({ label: t("protect.sign"), checked: !!on?.sign, disabled: !canSign, hint: canSign ? undefined : t("protect.noOwnKey"), onSelect: () => set(!on?.sign, !!on?.encrypt) });
      items.push({
        label: t("protect.encrypt"),
        checked: !!on?.encrypt,
        disabled: !canEncrypt,
        hint: canEncrypt ? undefined : missing.length ? t("protect.missingFor", { n: missing.length }) : t("protect.noRecipients"),
        onSelect: () => set(on?.sign ?? canSign, !on?.encrypt),
      });
    }
    if (protect) items.push("separator", { label: t("protect.none"), onSelect: () => onChange({ ...draft, protect: null }) });
    return items;
  };
  const [dropping, setDropping] = useState(false);
  // „Erinnern, wenn keine Antwort“ after this many days (null: no reminder).
  const [remind, setRemind] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  // A phone: the caret is in the text (the formatting bar shows over the keyboard).
  const [writing, setWriting] = useState(false);
  const followupSecs = remind == null ? null : remind * 86400;
  const above = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    return { x: r.left, y: Math.max(8, r.top - 8 - 220) };
  };
  const sheet = useRef<HTMLDivElement>(null);
  const subject = useRef<HTMLInputElement>(null);
  const body = useRef<HTMLTextAreaElement>(null);
  const latest = useRef(draft);
  latest.current = draft;
  const set = (patch: Partial<ComposeState>) => onChange({ ...draft, ...patch });
  /** A change on top of the latest one (two in one go, a chip and the cleared typing, both stay). */
  const update = (patch: Partial<ComposeState>) => {
    latest.current = { ...latest.current, ...patch };
    onChange(latest.current);
  };
  const typing = draft.typing ?? { to: "", cc: "", bcc: "" };
  const setTyping = (field: "to" | "cc" | "bcc", v: string) => update({ typing: { ...(latest.current.typing ?? { to: "", cc: "", bcc: "" }), [field]: v } });
  const [templates, setTemplates] = useState<Template[]>([]);
  const [snip, setSnip] = useState<SnippetPick | null>(null);
  const help = useAi();
  const [helpStyle, setHelpStyle] = useState<WritingStyle | null>(null);
  const [dest, setDest] = useState<Destination | null>(null);
  useEffect(() => {
    void api.templates().then(setTemplates);
  }, []);
  useEffect(() => {
    if (aiOn && draft.accountId != null) void api.aiDestination(draft.accountId).then(setDest);
  }, [aiOn, draft.accountId]);

  /** Fills a template's or snippet's variables for this message. */
  const render = async (texts: string[]) => {
    const d = latest.current;
    const now = today(lang);
    return api.templateRender(texts, d.accountId, d.to, d.subject, now.date, now.time);
  };

  const insertTemplate = async (tpl: Template) => {
    const [subject, filled] = await render([tpl.subject, tpl.body]);
    const d = latest.current;
    // The account's signature is already below the text: a template's {signatur} adds it once only.
    const sig = accounts.find((a) => a.id === d.accountId)?.signature.trim() ?? "";
    const firstLine = sig.split("\n")[0];
    const body = sig && firstLine && (d.rich ? d.html : d.text).includes(firstLine) ? filled.replace(sig, "").trimEnd() : filled;
    const own = ownText(d);
    const text = own ? `${body}\n\n${own}` : body;
    onChange({ ...replaceOwnText(d, text), subject: d.subject.trim() ? d.subject : subject });
  };

  /** Puts the chosen snippet in place of `;;query`. */
  const insertSnippet = async (tpl: Template, query: string) => {
    setSnip(null);
    const [text] = await render([tpl.body]);
    const typed = query.length + 2;
    if (latest.current.rich) {
      const sel = window.getSelection();
      const node = sel?.anchorNode;
      if (sel && node && node.nodeType === Node.TEXT_NODE) {
        const r = document.createRange();
        r.setStart(node, Math.max(0, sel.anchorOffset - typed));
        r.setEnd(node, sel.anchorOffset);
        sel.removeAllRanges();
        sel.addRange(r);
        document.execCommand("insertText", false, text);
        const ed = sheet.current?.querySelector<HTMLElement>(".editor");
        if (ed) onChange({ ...latest.current, html: ed.innerHTML });
      }
      return;
    }
    const ta = body.current;
    if (!ta) return;
    const pos = ta.selectionStart;
    const value = latest.current.text;
    const next = value.slice(0, pos - typed) + text + value.slice(pos);
    onChange({ ...latest.current, text: next });
    requestAnimationFrame(() => {
      ta.focus();
      ta.setSelectionRange(pos - typed + text.length, pos - typed + text.length);
    });
  };

  const typed = (before: string) => {
    const q = snippetQuery(before);
    if (q == null) return setSnip(null);
    const list = matchSnippets(templates, q, latest.current.accountId);
    setSnip(list.length ? { query: q, list, active: 0 } : null);
  };

  /** Arrow keys, Enter and Tab in the snippet chooser. */
  const snippetKey = (e: KeyboardEvent<HTMLElement>): boolean => {
    if (!snip) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      setSnip({ ...snip, active: (snip.active + (e.key === "ArrowDown" ? 1 : -1) + snip.list.length) % snip.list.length });
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.stopPropagation();
      void insertSnippet(snip.list[snip.active], snip.query);
      return true;
    }
    if (e.key === "Escape") {
      e.stopPropagation();
      setSnip(null);
      return true;
    }
    return false;
  };

  const writingHelp = (style: WritingStyle) => {
    const text = ownText(latest.current);
    setHelpStyle(style);
    if (draft.accountId == null) return;
    void help.run({ task: "rewrite", account_id: draft.accountId, text, style });
  };

  const attach = async (paths: string[]) => {
    const infos = await api.fileInfos(paths);
    const cur = latest.current;
    const known = new Set(cur.attachments.map((a) => a.path));
    onChange({ ...cur, attachments: [...cur.attachments, ...infos.filter((i) => !known.has(i.path))] });
  };

  useEffect(() => onFileDrop((paths) => void attach(paths), setDropping), []);

  useEffect(() => {
    // Replies start in the body (above the quote), new messages in To.
    if (draft.mode === "reply" || draft.mode === "replyAll") {
      if (draft.rich) {
        const ed = sheet.current?.querySelector<HTMLElement>(".editor");
        ed?.focus();
        const sel = window.getSelection();
        if (ed && sel) {
          const r = document.createRange();
          r.setStart(ed, 0);
          r.collapse(true);
          sel.removeAllRanges();
          sel.addRange(r);
        }
      } else {
        body.current?.focus();
        body.current?.setSelectionRange(0, 0);
      }
    }
  }, []);

  const toggleRich = () => {
    if (draft.rich) set({ rich: false, text: editorHtmlToText(draft.html), revision: draft.revision + 1 });
    else set({ rich: true, html: textToEditorHtml(draft.text), revision: draft.revision + 1 });
  };

  const laterItems = (): MenuItem[] => sendLaterItems(t, (at) => onSendLater(at, followupSecs), () => onSendLater(null, followupSecs));
  const remindItems = (): MenuItem[] => followupItems(t, remind ?? 0, (days) => setRemind(days), null, remind != null ? () => setRemind(null) : null);
  const templateItems = (): MenuItem[] => {
    const list = templates.filter((x) => x.kind === "template" && (x.account_id == null || x.account_id === draft.accountId));
    const snippets = templates.filter((x) => x.kind === "snippet");
    return [
      ...(list.length ? list.map((x) => ({ label: x.name, onSelect: () => void insertTemplate(x) })) : [{ label: t("compose.noTemplates"), onSelect: () => {}, disabled: true }]),
      ...(snippets.length ? ["separator" as const, { heading: t("compose.snippetHint") }] : []),
    ];
  };
  const helpItems = (): MenuItem[] => (Object.keys(STYLE) as WritingStyle[]).map((s) => ({ label: t(STYLE[s]), onSelect: () => writingHelp(s) }));
  const canProtect = !!(abilities?.pgp_sign || abilities?.smime_sign || abilities?.pgp_encrypt || abilities?.smime_encrypt || protect);
  /** A phone's „more“ sheet: what the desktop's footer offers, each opening its own sheet. */
  const moreItems = (): MenuItem[] => {
    const open = (items: () => MenuItem[]) => () => setMenu({ items: items(), at: { x: 0, y: 0 } });
    const items: MenuItem[] = [
      { label: t("compose.sendLater"), icon: <Send size={16} strokeWidth={1.75} aria-hidden />, disabled: sending || draft.accountId == null, onSelect: open(laterItems) },
      { label: remind != null ? t("compose.remindSet", { n: remind }) : t("compose.remind"), icon: <AlarmClock size={16} strokeWidth={1.75} aria-hidden />, onSelect: open(remindItems) },
    ];
    if (canProtect) items.push({ label: t("protect.menu"), icon: <ShieldCheck size={16} strokeWidth={1.75} aria-hidden />, onSelect: open(protectItems) });
    items.push(
      { label: draft.rich ? t("compose.toPlain") : t("compose.toRich"), icon: <Type size={16} strokeWidth={1.75} aria-hidden />, onSelect: toggleRich },
      { label: t("compose.templates"), icon: <FileText size={16} strokeWidth={1.75} aria-hidden />, onSelect: open(templateItems) },
    );
    if (aiOn) items.push({ label: t("compose.writingHelp"), icon: <Sparkles size={16} strokeWidth={1.75} aria-hidden />, disabled: !dest?.provider, onSelect: open(helpItems) });
    items.push("separator", { label: t("compose.discard"), icon: <Trash2 size={16} strokeWidth={1.75} aria-hidden />, danger: true, onSelect: onDiscard });
    return items;
  };

  const account = accounts.find((a) => a.id === draft.accountId);
  // Encrypting was chosen, but the recipients as they are now cannot all be encrypted to (a
  // recipient without a key was added): said here, before sending fails.
  const recipientCount = draft.to.length + draft.cc.length + draft.bcc.length;
  const cannotEncrypt =
    protect?.encrypt && abilities && recipientCount > 0 && !(protect.method === "pgp" ? abilities.pgp_encrypt : abilities.smime_encrypt)
      ? protect.method === "pgp"
        ? abilities.pgp_missing
        : abilities.smime_missing
      : null;
  return (
    <div
      ref={sheet}
      className="compose"
      role="dialog"
      aria-modal={modal ? "true" : "false"}
      aria-label={t(TITLE[draft.mode])}
      data-dropping={dropping || undefined}
      data-phone={phone || undefined}
      data-writing={(phone && writing) || undefined}
      onKeyDown={(e) => {
        // A dialog the sheet opened (the link prompt) has its own keys.
        if ((e.target as HTMLElement).closest?.(".overlay")) return;
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          onSend(followupSecs);
        }
        if (e.key.toLowerCase() === "s" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          onSaveDraft();
        }
        if (e.key === "Tab" && modal) trapFocus(e, sheet.current);
      }}
    >
      {phone ? (
        <header className="compose-head compose-bar">
          <button type="button" className="icon-btn" onClick={onClose} aria-label={t("compose.close")}>
            <ArrowLeft size={20} strokeWidth={1.75} aria-hidden />
          </button>
          <h2 dir="auto">{draft.subject.trim() || t(TITLE[draft.mode])}</h2>
          {saved && (
            <span className="muted compose-saved" role="status">
              {t("compose.saved")}
            </span>
          )}
          <button type="button" className="icon-btn" onClick={() => void pickFiles().then((p) => (p.length ? attach(p) : undefined))} aria-label={t("compose.attach")}>
            <Paperclip size={20} strokeWidth={1.75} aria-hidden />
          </button>
          <button type="button" className="icon-btn compose-send" onClick={() => onSend(followupSecs)} disabled={sending || draft.accountId == null} aria-label={t("compose.send")}>
            {sending ? <Spinner /> : <Send size={18} strokeWidth={1.75} aria-hidden />}
          </button>
          <button type="button" className="icon-btn" aria-haspopup="menu" aria-label={t("read.more")} data-compose="more" onClick={() => setMenu({ items: moreItems(), at: { x: 0, y: 0 } })}>
            <MoreHorizontal size={20} strokeWidth={1.75} aria-hidden />
          </button>
        </header>
      ) : (
        <header className="compose-head">
          <h2 dir="auto">{draft.subject.trim() || t(TITLE[draft.mode])}</h2>
          <button type="button" className="icon-btn" onClick={onClose} aria-label={t("compose.close")} title={t("compose.close")}>
            <X size={16} strokeWidth={1.75} aria-hidden />
          </button>
        </header>
      )}
      <div className="compose-fields">
        {accounts.length > 1 && (
          <div className="field">
            <label htmlFor="compose-from">{t("compose.from")}</label>
            <select id="compose-from" className="select select-bare" value={draft.accountId ?? ""} onChange={(e) => set({ accountId: Number(e.target.value) })}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.sender_name ? `${a.sender_name} <${a.email}>` : a.email}
                </option>
              ))}
            </select>
          </div>
        )}
        <AddressField
          label={t("compose.to")}
          values={draft.to}
          text={typing.to}
          onText={(v) => setTyping("to", v)}
          onChange={(to) => update({ to })}
          autoFocus={draft.mode === "new" || draft.mode === "forward"}
          extra={
            phone ? (
              !draft.showCc &&
              !draft.showBcc && (
                <span className="field-extra">
                  <button type="button" className="btn btn-sm btn-quiet compose-ccbcc" onClick={() => set({ showCc: true, showBcc: true })}>
                    {t("compose.ccBcc")}
                  </button>
                </span>
              )
            ) : (
              <span className="field-extra">
                {!draft.showCc && (
                  <button type="button" className="link-btn" onClick={() => set({ showCc: true })}>
                    {t("compose.cc")}
                  </button>
                )}
                {!draft.showBcc && (
                  <button type="button" className="link-btn" onClick={() => set({ showBcc: true })}>
                    {t("compose.bcc")}
                  </button>
                )}
              </span>
            )
          }
        />
        {draft.showCc && <AddressField label={t("compose.cc")} values={draft.cc} text={typing.cc} onText={(v) => setTyping("cc", v)} onChange={(cc) => update({ cc })} />}
        {draft.showBcc && <AddressField label={t("compose.bcc")} values={draft.bcc} text={typing.bcc} onText={(v) => setTyping("bcc", v)} onChange={(bcc) => update({ bcc })} />}
        <div className="field">
          <label htmlFor="compose-subject">{t("compose.subject")}</label>
          <input id="compose-subject" ref={subject} className="input input-bare" dir="auto" value={draft.subject} onChange={(e) => set({ subject: e.target.value })} />
        </div>
      </div>
      <div
        className="compose-body-wrap"
        onFocus={(e) => setWriting((e.target as HTMLElement).matches(".editor, .compose-body"))}
        onBlur={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setWriting(false);
        }}
      >
        {draft.rich ? (
          <Editor html={draft.html} revision={draft.revision} label={t("compose.body")} onChange={(html) => set({ html })} onTyped={typed} onKey={snippetKey} />
        ) : (
          <textarea
            ref={body}
            className="compose-body"
            dir="auto"
            aria-label={t("compose.body")}
            value={draft.text}
            onChange={(e) => {
              set({ text: e.target.value });
              typed(e.target.value.slice(0, e.target.selectionStart));
            }}
            onKeyDown={(e) => {
              if (snippetKey(e)) e.preventDefault();
            }}
          />
        )}
        {snip && (
          <ul className="snippet-pick" role="listbox" aria-label={t("tpl.snippets")}>
            {snip.list.map((s, i) => (
              <li
                key={s.id}
                role="option"
                aria-selected={i === snip.active}
                className="suggestion"
                onMouseDown={(e) => {
                  e.preventDefault();
                  void insertSnippet(s, snip.query);
                }}
              >
                <code className="snippet-trigger">;;{s.trigger}</code>
                <span className="suggestion-name">{s.name}</span>
                <span className="suggestion-email">{s.body.replace(/\s+/g, " ").slice(0, 60)}</span>
              </li>
            ))}
          </ul>
        )}
        {(draft.attachments.length > 0 || draft.forwarded.length > 0) && (
          <ul className="compose-attachments" aria-label={t("compose.attachments")}>
            {draft.forwarded.map((a) => {
              const name = a.filename ?? t("read.unnamed");
              return (
                <li key={`f${a.id}`} className="chip chip-file">
                  <File size={13} strokeWidth={1.75} aria-hidden />
                  <span className="chip-text">{name}</span>
                  <span className="chip-meta">{fileSize(a.size, lang)}</span>
                  <button
                    type="button"
                    className="chip-remove"
                    aria-label={t("compose.removeAttachment", { name })}
                    onClick={() => set({ forwarded: draft.forwarded.filter((x) => x.id !== a.id) })}
                  >
                    <X size={12} strokeWidth={2} aria-hidden />
                  </button>
                </li>
              );
            })}
            {draft.attachments.map((a) => (
              <li key={a.path} className="chip chip-file">
                <File size={13} strokeWidth={1.75} aria-hidden />
                <span className="chip-text">{a.name}</span>
                <span className="chip-meta">{fileSize(a.size, lang)}</span>
                <button
                  type="button"
                  className="chip-remove"
                  aria-label={t("compose.removeAttachment", { name: a.name })}
                  onClick={() => set({ attachments: draft.attachments.filter((x) => x.path !== a.path) })}
                >
                  <X size={12} strokeWidth={2} aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}
        {dropping && (
          <div className="drop-hint" aria-hidden>
            <Paperclip size={20} strokeWidth={1.75} />
            <span>{t("compose.dropHint")}</span>
          </div>
        )}
      </div>
      {helpStyle && (help.state.busy || help.state.text || help.state.error) && (
        <div className="writing-help" aria-label={t("compose.writingHelp")}>
          <header className="assistant-out-head">
            <span className="assistant-out-title">{t(STYLE[helpStyle])}</span>
            <PrivacyLine dest={dest} />
            <span className="toolbar-gap" />
            {help.state.busy && (
              <button type="button" className="btn btn-sm btn-quiet" onClick={help.cancel}>
                <Square size={12} strokeWidth={2} aria-hidden />
                <span>{t("ai.stop")}</span>
              </button>
            )}
          </header>
          {help.state.busy && !help.state.text && (
            <p className="muted assistant-wait">
              <Spinner /> {dest?.local ? t("ai.thinkingLocal") : t("ai.thinking")}
            </p>
          )}
          {help.state.text && <AiText text={help.state.text} busy={help.state.busy} />}
          {help.state.error && (
            <div className="notice notice-warning" role="alert">
              <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
              <span>{help.state.error}</span>
            </div>
          )}
          {!help.state.busy && (
            <div className="assistant-out-foot">
              {help.state.text && !help.state.error && (
                <button
                  type="button"
                  className="btn btn-sm btn-primary"
                  onClick={() => {
                    onChange(replaceOwnText(latest.current, help.state.text));
                    help.reset();
                    setHelpStyle(null);
                  }}
                >
                  <Check size={14} strokeWidth={1.75} aria-hidden />
                  <span>{t("compose.helpApply")}</span>
                </button>
              )}
              <button
                type="button"
                className="btn btn-sm btn-quiet"
                onClick={() => {
                  help.reset();
                  setHelpStyle(null);
                }}
              >
                {t("compose.helpDiscard")}
              </button>
            </div>
          )}
        </div>
      )}
      {draft.fromEncrypted && !protect?.encrypt && (
        <div className="notice notice-warning compose-quote-warning" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("protect.quotesEncrypted")}</span>
        </div>
      )}
      {cannotEncrypt && (
        <div className="notice notice-warning compose-cannot-encrypt" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
          <span>{cannotEncrypt.length ? t("protect.cannotEncryptFor", { n: cannotEncrypt.length }) : t("protect.cannotEncryptOwn")}</span>
        </div>
      )}
      {error && (
        <div className="notice notice-danger compose-error" role="alert">
          <strong>{error.title}</strong>
          <span>{error.detail}</span>
        </div>
      )}
      {!phone && (
        <footer className="compose-foot">
          <span className="split-btn">
            <button type="button" className="btn btn-primary" onClick={() => onSend(followupSecs)} disabled={sending || draft.accountId == null} title={t("compose.sendHint")}>
              <Send size={15} strokeWidth={1.75} aria-hidden />
              <span>{t("compose.send")}</span>
            </button>
            <button
              type="button"
              className="btn btn-primary split-btn-more"
              disabled={sending || draft.accountId == null}
              aria-label={t("compose.sendLater")}
              title={t("compose.sendLater")}
              aria-haspopup="menu"
              onClick={(e) => setMenu({ items: laterItems(), at: above(e.currentTarget) })}
            >
              <ChevronDown size={15} strokeWidth={1.75} aria-hidden />
            </button>
          </span>
          <button
            type="button"
            className={`btn btn-quiet remind-btn${remind != null ? " remind-on" : ""}`}
            aria-pressed={remind != null}
            aria-haspopup="menu"
            aria-label={remind != null ? t("compose.remindSet", { n: remind }) : t("compose.remind")}
            title={t("compose.remind")}
            onClick={(e) => setMenu({ items: remindItems(), at: above(e.currentTarget) })}
          >
            <AlarmClock size={16} strokeWidth={1.75} aria-hidden />
            {remind != null && <span>{t("compose.remindSet", { n: remind })}</span>}
          </button>
          <button type="button" className="icon-btn" onClick={() => void pickFiles().then((p) => (p.length ? attach(p) : undefined))} aria-label={t("compose.attach")} title={t("compose.attach")}>
            <Paperclip size={16} strokeWidth={1.75} aria-hidden />
          </button>
          {canProtect && (
            <button
              type="button"
              className={`btn btn-quiet protect-btn${protect ? " protect-on" : ""}`}
              data-compose="protect"
              aria-haspopup="menu"
              aria-pressed={!!protect}
              aria-label={t("protect.menu")}
              title={t("protect.menu")}
              onClick={(e) => setMenu({ items: protectItems(), at: above(e.currentTarget) })}
            >
              {protect?.encrypt ? <Lock size={16} strokeWidth={1.75} aria-hidden /> : <ShieldCheck size={16} strokeWidth={1.75} aria-hidden />}
              {protect && <span>{protect.encrypt ? t("protect.encryptedShort", { method: protect.method === "pgp" ? "OpenPGP" : "S/MIME" }) : t("protect.signedShort", { method: protect.method === "pgp" ? "OpenPGP" : "S/MIME" })}</span>}
            </button>
          )}
          <button type="button" className="icon-btn" aria-pressed={draft.rich} onClick={toggleRich} aria-label={draft.rich ? t("compose.toPlain") : t("compose.toRich")} title={draft.rich ? t("compose.toPlain") : t("compose.toRich")}>
            <Type size={16} strokeWidth={1.75} aria-hidden />
          </button>
          <button
            type="button"
            className="icon-btn"
            data-compose="templates"
            aria-haspopup="menu"
            aria-label={t("compose.templates")}
            title={t("compose.templates")}
            onClick={(e) => setMenu({ items: templateItems(), at: above(e.currentTarget) })}
          >
            <FileText size={16} strokeWidth={1.75} aria-hidden />
          </button>
          {aiOn && (
            <button
              type="button"
              className="icon-btn"
              data-compose="help"
              aria-haspopup="menu"
              disabled={!dest?.provider}
              aria-label={t("compose.writingHelp")}
              title={dest?.provider ? t("compose.writingHelp") : t("ai.notAllowed")}
              onClick={(e) => setMenu({ items: helpItems(), at: above(e.currentTarget) })}
            >
              <Sparkles size={16} strokeWidth={1.75} aria-hidden />
            </button>
          )}
          <span className="toolbar-gap" />
          {saved && (
            <span className="muted compose-saved" role="status">
              {t("compose.saved")}
            </span>
          )}
          <span className="muted compose-account">{account?.email}</span>
          <button type="button" className="btn btn-quiet" onClick={onSaveDraft}>
            {t("compose.saveDraft")}
          </button>
          <button type="button" className="icon-btn" onClick={onDiscard} aria-label={t("compose.discard")} title={t("compose.discard")}>
            <Trash2 size={16} strokeWidth={1.75} aria-hidden />
          </button>
        </footer>
      )}
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("compose.sendLater")} />}
    </div>
  );
}
