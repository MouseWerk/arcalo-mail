import { CircleAlert, Clock, Menu as MenuIcon, RefreshCw, Trash2, Upload } from "lucide-react";
import { currentLang, useT, type TKey } from "../lib/i18n";
import type { AccountInfo, OutboxEntry } from "../lib/types";

const KIND: Record<string, TKey> = {
  send: "outbox.send",
  append_sent: "outbox.appendSent",
  flags: "outbox.flags",
  keyword: "outbox.keyword",
  move: "outbox.move",
  delete: "outbox.delete",
  save_draft: "outbox.saveDraft",
  delete_draft: "outbox.deleteDraft",
  create_folder: "outbox.createFolder",
  rename_folder: "outbox.renameFolder",
  delete_folder: "outbox.deleteFolder",
};

/** The offline queue: what waits for the server, and what the server refused. */
export function Outbox({ entries, accounts, onRetry, onDiscard, onShowFolders }: { entries: OutboxEntry[]; accounts: AccountInfo[]; onRetry: (id: number) => void; onDiscard: (id: number) => void; onShowFolders: () => void }) {
  const t = useT();
  const lang = currentLang();
  const time = (s: number) => new Date(s * 1000).toLocaleTimeString(lang === "de" ? "de-DE" : "en-GB", { hour: "2-digit", minute: "2-digit" });
  return (
    <section className="list-pane outbox" aria-label={t("folder.outbox")}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{t("folder.outbox")}</h1>
          <span className="muted list-count">{t("outbox.count", { n: entries.length })}</span>
        </div>
        <p className="muted outbox-note">{t("outbox.note")}</p>
      </header>
      <ul className="outbox-list">
        {entries.map((e) => {
          const account = accounts.find((a) => a.id === e.account_id);
          const parked = e.state === "parked";
          return (
            <li key={e.id} className="outbox-item" data-state={e.state}>
              <span className="outbox-icon">
                {parked || e.state === "failed" ? <CircleAlert size={16} strokeWidth={1.75} aria-hidden /> : e.kind === "send" ? <Upload size={16} strokeWidth={1.75} aria-hidden /> : <Clock size={16} strokeWidth={1.75} aria-hidden />}
              </span>
              <div className="outbox-text">
                <div className="outbox-title">{t(KIND[e.kind] ?? "outbox.other")}</div>
                <div className="outbox-meta muted">
                  {account?.name || account?.email}
                  {" · "}
                  {parked
                    ? t("outbox.refused")
                    : e.state === "failed"
                      ? t(e.kind === "send" ? "outbox.sendWaits" : "outbox.retryAt", { time: time(e.next_try) })
                      : e.state === "running"
                        ? t("outbox.running")
                        : t("outbox.waiting")}
                </div>
                {e.last_error && <div className="outbox-error">{e.last_error}</div>}
              </div>
              <button type="button" className="icon-btn" onClick={() => onRetry(e.id)} aria-label={t("common.retry")} title={t("common.retry")}>
                <RefreshCw size={15} strokeWidth={1.75} aria-hidden />
              </button>
              <button type="button" className="icon-btn" onClick={() => onDiscard(e.id)} aria-label={t("outbox.discard")} title={t("outbox.discard")}>
                <Trash2 size={15} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
