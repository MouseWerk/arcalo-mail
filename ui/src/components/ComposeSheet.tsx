import {
  Bold,
  File,
  Italic,
  Link,
  List,
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
import { editorHtmlToText, isValidAddress, splitAddresses, textToEditorHtml, type ComposeState } from "../lib/compose";
import { api, onFileDrop, pickFiles } from "../lib/api";
import { fileSize } from "../lib/format";
import type { Account, Contact } from "../lib/types";
import { PromptDialog, trapFocus } from "./ui";

type Props = {
  draft: ComposeState;
  accounts: Account[];
  error: string | null;
  sending: boolean;
  onChange: (d: ComposeState) => void;
  onSend: () => void;
  onSaveDraft: () => void;
  onClose: () => void;
  onDiscard: () => void;
};

const TITLE: Record<ComposeState["mode"], TKey> = {
  new: "compose.title",
  reply: "compose.reply",
  replyAll: "compose.replyAll",
  forward: "compose.forward",
  draft: "compose.draft",
};

/** To, Cc or Bcc: addresses as chips, suggestions from the address book while typing. */
function AddressField({
  label,
  values,
  onChange,
  autoFocus,
  extra,
}: {
  label: string;
  values: string[];
  onChange: (v: string[]) => void;
  autoFocus?: boolean;
  extra?: React.ReactNode;
}) {
  const t = useT();
  const [text, setText] = useState("");
  const [hits, setHits] = useState<Contact[]>([]);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const id = useRef(`addr-${Math.random().toString(36).slice(2)}`).current;

  useEffect(() => {
    if (autoFocus) input.current?.focus();
  }, []);

  useEffect(() => {
    const q = text.trim();
    if (!q) {
      setHits([]);
      return;
    }
    const timer = setTimeout(() => {
      void api.suggest(q).then((list) => {
        setHits(list.filter((c) => !values.some((v) => v.toLowerCase().includes(c.email.toLowerCase()))));
        setActive(0);
      });
    }, 100);
    return () => clearTimeout(timer);
  }, [text]);

  const commit = (raw: string) => {
    const parts = splitAddresses(raw);
    if (parts.length) onChange([...values, ...parts]);
    setText("");
    setHits([]);
  };
  const pick = (c: Contact) => commit(c.name ? `${/[,;"]/.test(c.name) ? `"${c.name.replace(/"/g, "")}"` : c.name} <${c.email}>` : c.email);

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
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
            <span className="chip-text">{v.replace(/\s*<[^>]+>$/, "") || v}</span>
            <button type="button" className="chip-remove" aria-label={t("compose.removeRecipient", { name: v })} onClick={() => onChange(values.filter((_, j) => j !== i))}>
              <X size={12} strokeWidth={2} aria-hidden />
            </button>
          </span>
        ))}
        <input
          ref={input}
          id={id}
          className="chip-input"
          value={text}
          role="combobox"
          aria-expanded={hits.length > 0}
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={hits.length ? `${id}-${active}` : undefined}
          onChange={(e) => {
            const v = e.target.value;
            if (/[,;]/.test(v)) commit(v);
            else setText(v);
          }}
          onKeyDown={onKey}
          onBlur={() => setTimeout(() => {
            if (text.trim()) commit(text);
            setHits([]);
          }, 120)}
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
                <span className="suggestion-name">{c.name || c.email}</span>
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

/** The rich text editor: a content-editable area with a few formats. */
function Editor({ html, revision, onChange, label }: { html: string; revision: number; onChange: (h: string) => void; label: string }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [linking, setLinking] = useState(false);
  const saved = useRef<Range | null>(null);
  useEffect(() => {
    if (ref.current && ref.current.innerHTML !== html) ref.current.innerHTML = html || "<p><br></p>";
  }, [revision]);
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
        {tool(t("compose.link"), <Link size={15} strokeWidth={2} aria-hidden />, () => {
          const sel = window.getSelection();
          saved.current = sel && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
          setLinking(true);
        })}
        {tool(t("compose.clearFormat"), <RemoveFormatting size={15} strokeWidth={2} aria-hidden />, () => run("removeFormat"))}
      </div>
      <div
        ref={ref}
        className="editor"
        contentEditable
        role="textbox"
        aria-multiline="true"
        aria-label={label}
        suppressContentEditableWarning
        onInput={() => ref.current && onChange(ref.current.innerHTML)}
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
export function ComposeSheet({ draft, accounts, error, sending, onChange, onSend, onSaveDraft, onClose, onDiscard }: Props) {
  const t = useT();
  const lang = currentLang();
  const [dropping, setDropping] = useState(false);
  const sheet = useRef<HTMLDivElement>(null);
  const subject = useRef<HTMLInputElement>(null);
  const body = useRef<HTMLTextAreaElement>(null);
  const set = (patch: Partial<ComposeState>) => onChange({ ...draft, ...patch });
  const latest = useRef(draft);
  latest.current = draft;

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

  const account = accounts.find((a) => a.id === draft.accountId);
  return (
    <div
      ref={sheet}
      className="compose"
      role="dialog"
      aria-modal="false"
      aria-label={t(TITLE[draft.mode])}
      data-dropping={dropping || undefined}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          onSend();
        }
        if (e.key.toLowerCase() === "s" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          onSaveDraft();
        }
        if (e.key === "Tab") trapFocus(e, sheet.current);
      }}
    >
      <header className="compose-head">
        <h2>{draft.subject.trim() || t(TITLE[draft.mode])}</h2>
        <button type="button" className="icon-btn" onClick={onClose} aria-label={t("compose.close")} title={t("compose.close")}>
          <X size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </header>
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
          onChange={(to) => set({ to })}
          autoFocus={draft.mode === "new" || draft.mode === "forward"}
          extra={
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
          }
        />
        {draft.showCc && <AddressField label={t("compose.cc")} values={draft.cc} onChange={(cc) => set({ cc })} />}
        {draft.showBcc && <AddressField label={t("compose.bcc")} values={draft.bcc} onChange={(bcc) => set({ bcc })} />}
        <div className="field">
          <label htmlFor="compose-subject">{t("compose.subject")}</label>
          <input id="compose-subject" ref={subject} className="input input-bare" value={draft.subject} onChange={(e) => set({ subject: e.target.value })} />
        </div>
      </div>
      <div className="compose-body-wrap">
        {draft.rich ? (
          <Editor html={draft.html} revision={draft.revision} label={t("compose.body")} onChange={(html) => set({ html })} />
        ) : (
          <textarea ref={body} className="compose-body" aria-label={t("compose.body")} value={draft.text} onChange={(e) => set({ text: e.target.value })} />
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
      {error && (
        <div className="notice notice-danger compose-error" role="alert">
          <strong>{t("compose.sendFailed")}</strong>
          <span>{error}</span>
        </div>
      )}
      <footer className="compose-foot">
        <button type="button" className="btn btn-primary" onClick={onSend} disabled={sending || draft.accountId == null} title={t("compose.sendHint")}>
          <Send size={15} strokeWidth={1.75} aria-hidden />
          <span>{t("compose.send")}</span>
        </button>
        <button type="button" className="icon-btn" onClick={() => void pickFiles().then((p) => (p.length ? attach(p) : undefined))} aria-label={t("compose.attach")} title={t("compose.attach")}>
          <Paperclip size={16} strokeWidth={1.75} aria-hidden />
        </button>
        <button type="button" className="icon-btn" aria-pressed={draft.rich} onClick={toggleRich} aria-label={draft.rich ? t("compose.toPlain") : t("compose.toRich")} title={draft.rich ? t("compose.toPlain") : t("compose.toRich")}>
          <Type size={16} strokeWidth={1.75} aria-hidden />
        </button>
        <span className="toolbar-gap" />
        <span className="muted compose-account">{account?.email}</span>
        <button type="button" className="btn btn-quiet" onClick={onSaveDraft}>
          {t("compose.saveDraft")}
        </button>
        <button type="button" className="icon-btn" onClick={onDiscard} aria-label={t("compose.discard")} title={t("compose.discard")}>
          <Trash2 size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </footer>
    </div>
  );
}
