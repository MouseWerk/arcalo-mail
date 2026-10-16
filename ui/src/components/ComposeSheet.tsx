import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useT, type TKey } from "../lib/i18n";
import { splitAddresses, type ComposeState } from "../lib/compose";
import { api } from "../lib/api";
import type { Account } from "../lib/types";

type Props = {
  draft: ComposeState;
  accounts: Account[];
  onChange: (d: ComposeState) => void;
  onClose: () => void;
};

const TITLE: Record<ComposeState["mode"], TKey> = {
  new: "compose.title",
  reply: "compose.reply",
  replyAll: "compose.reply",
  forward: "compose.forward",
};

/** The compose sheet: bottom right on a wide window, full screen on a phone. Esc closes it,
 *  Ctrl+Enter sends. */
export function ComposeSheet({ draft, accounts, onChange, onClose }: Props) {
  const t = useT();
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const first = useRef<HTMLInputElement>(null);
  const body = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (draft.mode === "new" || draft.mode === "forward") first.current?.focus();
    else {
      body.current?.focus();
      body.current?.setSelectionRange(0, 0);
    }
    // Only when the sheet opens.
  }, []);

  const set = (patch: Partial<ComposeState>) => onChange({ ...draft, ...patch });

  const send = async () => {
    if (draft.accountId == null || sending) return;
    setSending(true);
    setError(null);
    try {
      await api.send({ account_id: draft.accountId, to: splitAddresses(draft.to), cc: splitAddresses(draft.cc), subject: draft.subject, body: draft.body });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      className="compose"
      role="dialog"
      aria-modal="false"
      aria-label={t(TITLE[draft.mode])}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void send();
      }}
    >
      <header className="compose-head">
        <h2>{t(TITLE[draft.mode])}</h2>
        <button type="button" className="icon-btn" onClick={onClose} aria-label={t("common.close")}>
          <X size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </header>
      <div className="compose-fields">
        <label className="field">
          <span>{t("compose.from")}</span>
          <select className="select" value={draft.accountId ?? ""} onChange={(e) => set({ accountId: Number(e.target.value) })}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.sender_name ? `${a.sender_name} <${a.email}>` : a.email}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>{t("compose.to")}</span>
          <input ref={first} className="input" value={draft.to} onChange={(e) => set({ to: e.target.value })} />
        </label>
        <label className="field">
          <span>{t("compose.cc")}</span>
          <input className="input" value={draft.cc} onChange={(e) => set({ cc: e.target.value })} />
        </label>
        <label className="field">
          <span>{t("compose.subject")}</span>
          <input className="input" value={draft.subject} onChange={(e) => set({ subject: e.target.value })} />
        </label>
      </div>
      <textarea
        ref={body}
        className="compose-body"
        aria-label={t("compose.body")}
        value={draft.body}
        onChange={(e) => set({ body: e.target.value })}
      />
      {error && (
        <div className="notice notice-danger" role="alert">
          <strong>{t("compose.sendFailed")}</strong>
          <span>{error}</span>
        </div>
      )}
      <footer className="compose-foot">
        <button type="button" className="btn btn-primary" onClick={() => void send()} disabled={sending || draft.accountId == null}>
          {t("compose.send")}
        </button>
        <span className="muted">{t("compose.sendHint")}</span>
        <span className="toolbar-gap" />
        <button type="button" className="btn" onClick={onClose}>
          {t("compose.discard")}
        </button>
      </footer>
    </div>
  );
}
