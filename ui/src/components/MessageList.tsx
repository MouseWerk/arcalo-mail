import { Archive, Flag, Mail, MailOpen, Menu as MenuIcon, Paperclip, Search, Server, ShieldAlert, Trash2, X, FolderInput, Reply } from "lucide-react";
import { useEffect, useLayoutEffect, useState, type ReactNode } from "react";
import { currentLang, useT } from "../lib/i18n";
import { displayName, listDate } from "../lib/format";
import { visibleRange } from "../lib/listing";
import type { MessageSummary } from "../lib/types";
import { Spinner } from "./ui";

export type BulkAction = "archive" | "delete" | "spam" | "read" | "unread" | "flag" | "move";

type Props = {
  title: string;
  total: number;
  rowAt: (index: number) => MessageSummary | undefined;
  onRange: (start: number, end: number) => void;
  selected: number[];
  current: number | null;
  query: string;
  searching: boolean;
  /** „Auf dem Server suchen“: offered while searching; `busy` while it runs. */
  serverSearch: { busy: boolean; done: boolean } | null;
  showSnippets: boolean;
  loading: boolean;
  emptyText: string;
  onQuery: (q: string) => void;
  onServerSearch: () => void;
  onRowClick: (index: number, mods: { shift: boolean; toggle: boolean }) => void;
  onBulk: (a: BulkAction, anchor?: DOMRect) => void;
  onClearSelection: () => void;
  /** Phones: back to the folders. */
  onShowFolders: () => void;
  searchRef: React.RefObject<HTMLInputElement | null>;
  listRef: React.RefObject<HTMLDivElement | null>;
  /** The row ids in order as far as loaded (for the keyboard). */
  extra?: ReactNode;
};

function tokenPx(name: string, fallback: number): number {
  if (typeof document === "undefined") return fallback;
  const v = parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name));
  return Number.isFinite(v) && v > 0 ? v : fallback;
}

export function MessageList(p: Props) {
  const t = useT();
  const lang = currentLang();
  const now = new Date();
  const scroller = p.listRef;
  const [range, setRange] = useState({ start: 0, end: 40 });
  const [rowH, setRowH] = useState(76);

  useLayoutEffect(() => {
    setRowH(p.showSnippets ? tokenPx("--row-h", 76) : tokenPx("--row-h-compact", 54));
  }, [p.showSnippets]);

  const measure = () => {
    const el = scroller.current;
    if (!el) return;
    const r = visibleRange(el.scrollTop, el.clientHeight, rowH, p.total);
    setRange((old) => (old.start === r.start && old.end === r.end ? old : r));
  };

  useEffect(measure, [p.total, rowH]);
  useEffect(() => p.onRange(range.start, range.end), [range.start, range.end, p.total]);
  useEffect(() => {
    const el = scroller.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  });

  // Keep the current message in view when the keyboard moves it.
  useEffect(() => {
    const el = scroller.current;
    if (!el || p.current == null) return;
    for (let i = range.start; i < range.end; i++) {
      if (p.rowAt(i)?.id !== p.current) continue;
      const top = i * rowH;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + rowH > el.scrollTop + el.clientHeight) el.scrollTop = top + rowH - el.clientHeight;
      break;
    }
  }, [p.current]);

  const rows: ReactNode[] = [];
  for (let i = range.start; i < range.end; i++) {
    const m = p.rowAt(i);
    const top = i * rowH;
    if (!m) {
      rows.push(<div key={`ph-${i}`} className="row row-placeholder" style={{ top, height: rowH }} aria-hidden />);
      continue;
    }
    const selected = p.selected.includes(m.id);
    const isCurrent = m.id === p.current;
    rows.push(
      <div
        key={m.id}
        id={`msg-${m.id}`}
        role="option"
        aria-selected={selected || isCurrent}
        aria-current={isCurrent ? "true" : undefined}
        data-id={m.id}
        data-unread={!m.seen || undefined}
        data-selected={selected && p.selected.length > 1 ? "" : undefined}
        className="row"
        style={{ top, height: rowH }}
        onMouseDown={(e) => {
          if (e.shiftKey) e.preventDefault();
        }}
        onClick={(e) => p.onRowClick(i, { shift: e.shiftKey, toggle: e.ctrlKey || e.metaKey })}
      >
        <span className="row-top">
          <span className="unread-slot">{!m.seen && <span className="unread-dot" aria-label={t("list.unread")} />}</span>
          <span className="row-from">{displayName(m.from, lang)}</span>
          {m.thread_count > 1 && (
            <span className="thread-count" aria-label={t("list.threadCount", { n: m.thread_count })}>
              {m.thread_count}
            </span>
          )}
          <span className="row-icons">
            {m.answered && <Reply size={12} strokeWidth={1.75} aria-label={t("list.answered")} />}
            {m.has_attachments && <Paperclip size={12} strokeWidth={1.75} aria-label={t("list.attachment")} />}
            {m.flagged && <Flag size={12} strokeWidth={1.75} className="flag" aria-label={t("list.flagged")} />}
          </span>
          <span className="row-date">{listDate(m.date, now, lang)}</span>
        </span>
        <span className="row-subject">{m.subject || t("list.noSubject")}</span>
        {p.showSnippets && <span className="row-snippet">{m.snippet || " "}</span>}
      </div>,
    );
  }

  const bulk = p.selected.length > 1;
  const bulkBtn = (a: BulkAction, icon: ReactNode, label: string) => (
    <button type="button" className="icon-btn" onClick={(e) => p.onBulk(a, e.currentTarget.getBoundingClientRect())} aria-label={label} title={label}>
      {icon}
    </button>
  );

  return (
    <section className="list-pane" aria-label={t("list.label")}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={p.onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{p.title}</h1>
          <span className="muted list-count">{p.searching ? t("list.results", { n: p.total }) : t("list.count", { n: p.total })}</span>
        </div>
        <div className="search">
          <Search size={16} strokeWidth={1.75} aria-hidden className="search-icon" />
          <input
            ref={p.searchRef}
            className="input search-input"
            type="search"
            value={p.query}
            placeholder={t("list.search")}
            aria-label={t("list.searchLabel")}
            onChange={(e) => p.onQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                p.onQuery("");
                p.listRef.current?.focus();
              }
              if (e.key === "ArrowDown" || e.key === "Enter") {
                e.preventDefault();
                p.listRef.current?.focus();
              }
            }}
          />
          {p.query && (
            <button type="button" className="icon-btn search-clear" onClick={() => p.onQuery("")} aria-label={t("list.clearSearch")}>
              <X size={14} strokeWidth={1.75} aria-hidden />
            </button>
          )}
        </div>
      </header>
      {bulk && (
        <div className="bulk-bar" role="toolbar" aria-label={t("list.bulkLabel")}>
          <span className="bulk-count">{t("list.selected", { n: p.selected.length })}</span>
          <span className="toolbar-gap" />
          {bulkBtn("archive", <Archive size={16} strokeWidth={1.75} aria-hidden />, t("read.archive"))}
          {bulkBtn("delete", <Trash2 size={16} strokeWidth={1.75} aria-hidden />, t("read.delete"))}
          {bulkBtn("spam", <ShieldAlert size={16} strokeWidth={1.75} aria-hidden />, t("read.spam"))}
          {bulkBtn("move", <FolderInput size={16} strokeWidth={1.75} aria-hidden />, t("read.move"))}
          {bulkBtn("read", <MailOpen size={16} strokeWidth={1.75} aria-hidden />, t("read.markRead"))}
          {bulkBtn("unread", <Mail size={16} strokeWidth={1.75} aria-hidden />, t("read.markUnread"))}
          {bulkBtn("flag", <Flag size={16} strokeWidth={1.75} aria-hidden />, t("read.flag"))}
          <button type="button" className="icon-btn" onClick={p.onClearSelection} aria-label={t("list.clearSelection")} title={t("list.clearSelection")}>
            <X size={16} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
      )}
      <div
        ref={scroller}
        className="messages"
        role="listbox"
        aria-label={p.title}
        aria-multiselectable="true"
        aria-activedescendant={p.current != null ? `msg-${p.current}` : undefined}
        tabIndex={0}
        onScroll={measure}
      >
        {p.total === 0 && !p.loading ? (
          <div className="empty">
            {p.searching ? <Search size={28} strokeWidth={1.5} aria-hidden /> : <Mail size={28} strokeWidth={1.5} aria-hidden />}
            <p>{p.emptyText}</p>
          </div>
        ) : (
          <div className="messages-inner" style={{ height: p.total * rowH }}>
            {rows}
          </div>
        )}
        {p.loading && p.total === 0 && (
          <div className="empty">
            <Spinner label={t("list.loading")} />
          </div>
        )}
      </div>
      {p.serverSearch && (
        <div className="list-foot">
          <button type="button" className="btn btn-sm" onClick={p.onServerSearch} disabled={p.serverSearch.busy || p.serverSearch.done}>
            {p.serverSearch.busy ? <Spinner /> : <Server size={14} strokeWidth={1.75} aria-hidden />}
            <span>{p.serverSearch.done ? t("list.serverSearched") : t("list.serverSearch")}</span>
          </button>
        </div>
      )}
      {p.extra}
    </section>
  );
}
