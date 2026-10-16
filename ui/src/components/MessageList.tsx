import { Flag, Menu, Paperclip, Search, X } from "lucide-react";
import { useRef, type KeyboardEvent } from "react";
import { currentLang, useT } from "../lib/i18n";
import { displayName, listDate } from "../lib/format";
import type { MessageSummary } from "../lib/types";

type Props = {
  title: string;
  messages: MessageSummary[];
  current: number | null;
  query: string;
  searching: boolean;
  onQuery: (q: string) => void;
  onOpen: (m: MessageSummary) => void;
  /** Phones: back to the folders. */
  onShowFolders: () => void;
};

export function MessageList({ title, messages, current, query, searching, onQuery, onOpen, onShowFolders }: Props) {
  const t = useT();
  const lang = currentLang();
  const now = new Date();
  const listRef = useRef<HTMLUListElement>(null);

  // Up and down move through the list (and open the message, as the reading pane follows).
  const onKey = (e: KeyboardEvent<HTMLUListElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const i = messages.findIndex((m) => m.id === current);
    const next = messages[Math.max(0, Math.min(messages.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
    if (!next) return;
    onOpen(next);
    listRef.current?.querySelector<HTMLButtonElement>(`[data-id="${next.id}"]`)?.focus();
  };

  return (
    <section className="list-pane" aria-label={t("list.label")}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <Menu size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{title}</h1>
          <span className="muted">{searching ? t("list.results", { n: messages.length }) : t("list.count", { n: messages.length })}</span>
        </div>
        <div className="search">
          <Search size={16} strokeWidth={1.75} aria-hidden className="search-icon" />
          <input
            className="input search-input"
            type="search"
            value={query}
            placeholder={t("list.search")}
            aria-label={t("list.searchLabel")}
            onChange={(e) => onQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && onQuery("")}
          />
          {query && (
            <button type="button" className="icon-btn search-clear" onClick={() => onQuery("")} aria-label={t("list.clearSearch")}>
              <X size={14} strokeWidth={1.75} aria-hidden />
            </button>
          )}
        </div>
      </header>
      {messages.length === 0 ? (
        <p className="empty">{searching ? t("list.noResults", { q: query.trim() }) : t("list.empty")}</p>
      ) : (
        <ul className="messages" ref={listRef} onKeyDown={onKey}>
          {messages.map((m) => (
            <li key={m.id}>
              <button
                type="button"
                data-id={m.id}
                className="row"
                data-unread={!m.seen || undefined}
                aria-current={m.id === current ? "true" : undefined}
                onClick={() => onOpen(m)}
              >
                <span className="row-top">
                  {!m.seen && <span className="unread-dot" aria-label={t("list.unread")} />}
                  <span className="row-from">{displayName(m.from, lang)}</span>
                  <span className="row-date">{listDate(m.date, now, lang)}</span>
                </span>
                <span className="row-subject">
                  <span className="row-subject-text">{m.subject || t("list.noSubject")}</span>
                  {m.has_attachments && <Paperclip size={13} strokeWidth={1.75} aria-label={t("list.attachment")} />}
                  {m.flagged && <Flag size={13} strokeWidth={1.75} className="flag" aria-label={t("list.flagged")} />}
                </span>
                <span className="row-snippet">{m.snippet}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
