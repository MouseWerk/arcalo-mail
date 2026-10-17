import { Archive, ArrowDown, Clock, Flag, FlagOff, Mail, MailOpen, Menu as MenuIcon, Paperclip, Pin, Search, Server, ShieldAlert, Trash2, X, FolderInput, Reply } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { currentLang, useT } from "../lib/i18n";
import { displayName, listDate, now } from "../lib/format";
import { MAX_LIST_PX, scrollWindow } from "../lib/listing";
import { PULL_TRIGGER, pullDistance, swipeIntent, swipeOffset, swipeRemoves, swipeResult, swipeSide } from "../lib/touch";
import type { MessageSummary, SwipeAction } from "../lib/types";
import { Avatar } from "./Avatar";
import { INSETS_EVENT } from "../lib/viewport";
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
  /** The sender's picture or initials at the start of each row (phones). */
  avatars?: boolean;
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
  /** Below the list (the phone's compose button). */
  extra?: ReactNode;
  /** Touch screens: swipes over rows and pull to refresh. */
  touch?: { left: SwipeAction; right: SwipeAction; onSwipe: (m: MessageSummary, a: SwipeAction) => void; onRefresh: () => Promise<void> };
  /** Below the search field: the inbox tabs, a note. */
  head?: ReactNode;
  /** Next to the title: the focus switch. */
  titleExtra?: ReactNode;
  /** Instead of the plain empty text (the calm inbox-zero). */
  empty?: ReactNode;
  /** The unified inbox: the account's color (1–6) and name for a row. */
  accountOf?: (m: MessageSummary) => { color: number; name: string } | null;
  /** The time tools' lists: the time shown instead of the date (when it comes back, is due). */
  dueOf?: (m: MessageSummary) => string | null;
};

/** A finger (or the mouse, for tests) on a row or at the top of the list. */
type Gesture = { id: number | null; x: number; y: number; atTop: boolean; mode: "swipe" | "scroll" | "pull" | null; pointer: number | null };

function SwipeLayer({ action, m, dir }: { action: SwipeAction; m: MessageSummary; dir: "left" | "right" }) {
  const t = useT();
  const [icon, label] =
    action === "archive"
      ? [<Archive key="i" size={18} strokeWidth={1.75} aria-hidden />, t("read.archive")]
      : action === "delete"
        ? [<Trash2 key="i" size={18} strokeWidth={1.75} aria-hidden />, t("read.delete")]
        : action === "flag"
          ? [m.flagged ? <FlagOff key="i" size={18} strokeWidth={1.75} aria-hidden /> : <Flag key="i" size={18} strokeWidth={1.75} aria-hidden />, m.flagged ? t("read.unflag") : t("read.flag")]
          : [m.seen ? <Mail key="i" size={18} strokeWidth={1.75} aria-hidden /> : <MailOpen key="i" size={18} strokeWidth={1.75} aria-hidden />, m.seen ? t("read.markUnread") : t("read.markRead")];
  return (
    <span className="row-swipe" data-action={action} data-dir={dir} aria-hidden>
      {icon}
      <span>{label}</span>
    </span>
  );
}

/** A size token in pixels, measured (touch screens compute theirs with the text size). */
function tokenPx(name: string, fallback: number): number {
  if (typeof document === "undefined" || !document.body) return fallback;
  const probe = document.createElement("div");
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  probe.style.height = `var(${name})`;
  document.body.appendChild(probe);
  const v = probe.getBoundingClientRect().height;
  probe.remove();
  return Number.isFinite(v) && v > 0 ? v : fallback;
}

export function MessageList(p: Props) {
  const t = useT();
  const lang = currentLang();
  const today = now();
  const scroller = p.listRef;
  const [range, setRange] = useState({ start: 0, end: 40, shift: 0, ratio: 1 });
  const [rowH, setRowH] = useState(76);
  const gesture = useRef<Gesture | null>(null);
  const swallowClick = useRef(false);
  const [swipe, setSwipe] = useState<{ id: number; dx: number; leaving?: boolean } | null>(null);
  const [pull, setPull] = useState<{ dy: number; busy: boolean }>({ dy: 0, busy: false });

  // After the app marked a touch screen (its layout effect runs before this one), and again
  // when Android's font size changes (the shell sets --type-scale, lib/viewport.ts).
  useEffect(() => {
    const measureRow = () => setRowH(p.showSnippets ? tokenPx("--row-h", 76) : tokenPx("--row-h-compact", 54));
    measureRow();
    window.addEventListener(INSETS_EVENT, measureRow);
    return () => window.removeEventListener(INSETS_EVENT, measureRow);
  }, [p.showSnippets, !!p.touch]);

  // Touch screens: scrolling down folds the compose button to its icon, scrolling up opens it.
  const [folded, setFolded] = useState(false);
  const lastTop = useRef(0);
  const measure = () => {
    const el = scroller.current;
    if (!el) return;
    if (p.touch) {
      const down = el.scrollTop > lastTop.current;
      if (Math.abs(el.scrollTop - lastTop.current) > 4) setFolded(down && el.scrollTop > rowH / 2);
      lastTop.current = el.scrollTop;
    }
    const r = scrollWindow(el.scrollTop, el.clientHeight, rowH, p.total);
    setRange((old) => (old.start === r.start && old.end === r.end && old.shift === r.shift && old.ratio === r.ratio ? old : r));
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
      const seen = el.scrollTop * range.ratio;
      if (top < seen) el.scrollTop = top / range.ratio;
      else if (top + rowH > seen + el.clientHeight) el.scrollTop = (top + rowH - el.clientHeight) / range.ratio;
      break;
    }
  }, [p.current]);

  // ---- touch: a sideways swipe acts on the row, a pull at the top checks for mail.
  const touch = p.touch;
  const width = () => scroller.current?.clientWidth ?? 360;
  const begin = (x: number, y: number, target: EventTarget | null, pointer: number | null) => {
    if (!touch || pull.busy) return;
    const row = (target as HTMLElement | null)?.closest?.<HTMLElement>(".row[data-id]");
    gesture.current = { id: row ? Number(row.dataset.id) : null, x, y, atTop: (scroller.current?.scrollTop ?? 0) <= 0, mode: null, pointer };
  };
  const move = (x: number, y: number): boolean => {
    const g = gesture.current;
    if (!g || !touch) return false;
    const dx = x - g.x;
    const dy = y - g.y;
    if (!g.mode) {
      const intent = swipeIntent(dx, dy);
      if (!intent) return false;
      g.mode = intent === "swipe" && g.id != null ? "swipe" : intent === "scroll" && g.atTop && dy > 0 ? "pull" : "scroll";
    }
    if (g.mode === "swipe" && g.id != null) {
      setSwipe({ id: g.id, dx: swipeOffset(dx, width(), touch.left, touch.right) });
      return true;
    }
    if (g.mode === "pull") {
      setPull({ dy: pullDistance(dy), busy: false });
      return true;
    }
    return false;
  };
  const end = (x: number) => {
    const g = gesture.current;
    gesture.current = null;
    if (!g || !touch) return;
    if (g.mode === "swipe" && g.id != null) {
      // The click that may follow the lifted finger is not a tap on the row.
      swallowClick.current = true;
      setTimeout(() => (swallowClick.current = false), 400);
      const dx = x - g.x;
      const action = swipeResult(dx, width(), touch.left, touch.right);
      const m = action ? Array.from({ length: range.end - range.start }, (_, k) => p.rowAt(range.start + k)).find((r) => r?.id === g.id) : undefined;
      if (action && m) {
        if (swipeRemoves(action)) {
          setSwipe({ id: g.id, dx: Math.sign(dx) * width(), leaving: true });
          setTimeout(() => {
            setSwipe(null);
            touch.onSwipe(m, action);
          }, 180);
        } else {
          setSwipe(null);
          touch.onSwipe(m, action);
        }
      } else setSwipe(null);
    } else if (g.mode === "pull") {
      if (pull.dy >= PULL_TRIGGER) {
        setPull({ dy: PULL_TRIGGER, busy: true });
        void touch.onRefresh().finally(() => setPull({ dy: 0, busy: false }));
      } else setPull({ dy: 0, busy: false });
    }
  };

  // Fingers: a pull must stop the page from scrolling, which only a non-passive listener can.
  useEffect(() => {
    const el = scroller.current;
    if (!el || !touch) return;
    const onMove = (e: TouchEvent) => {
      const t0 = e.touches[0];
      if (t0 && move(t0.clientX, t0.clientY) && gesture.current?.mode === "pull") e.preventDefault();
    };
    el.addEventListener("touchmove", onMove, { passive: false });
    return () => el.removeEventListener("touchmove", onMove);
  });

  const rows: ReactNode[] = [];
  for (let i = range.start; i < range.end; i++) {
    const m = p.rowAt(i);
    const top = i * rowH - range.shift;
    if (!m) {
      rows.push(<div key={`ph-${i}`} className="row row-placeholder" style={{ top, height: rowH }} aria-hidden />);
      continue;
    }
    const selected = p.selected.includes(m.id);
    const isCurrent = m.id === p.current;
    const account = p.accountOf?.(m) ?? null;
    const due = p.dueOf?.(m) ?? null;
    const bundle = m.bundle ?? 0;
    const sw = swipe?.id === m.id ? swipe : null;
    const side = sw && touch ? swipeSide(sw.dx, touch.left, touch.right) : null;
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
        data-swiping={sw ? "" : undefined}
        data-bundle={bundle > 0 ? "" : undefined}
        data-pinned={m.pinned ? "" : undefined}
        className="row"
        style={{ top, height: rowH }}
        onMouseDown={(e) => {
          if (e.shiftKey) e.preventDefault();
        }}
        onClick={(e) => {
          if (swallowClick.current) {
            swallowClick.current = false;
            return;
          }
          p.onRowClick(i, { shift: e.shiftKey, toggle: e.ctrlKey || e.metaKey });
        }}
      >
        {sw && side && <SwipeLayer action={side} m={m} dir={sw.dx < 0 ? "left" : "right"} />}
        <span className="row-body" style={sw ? { transform: `translateX(${sw.dx}px)`, transition: sw.leaving ? undefined : "none" } : undefined}>
          {p.avatars && <Avatar who={m.from} className="row-avatar" />}
          <span className="row-main">
            <span className="row-top">
              <span className="unread-slot">{!m.seen && <span className="unread-dot" aria-label={t("list.unread")} />}</span>
              <bdi className="row-from">{displayName(m.from, lang)}</bdi>
              {bundle > 1 ? (
                <span className="thread-count" aria-label={t("inbox.bundle", { n: bundle })}>
                  {bundle}
                </span>
              ) : (
                m.thread_count > 1 && (
                  <span className="thread-count" aria-label={t("list.threadCount", { n: m.thread_count })}>
                    {m.thread_count}
                  </span>
                )
              )}
              <span className="row-icons">
                {m.pinned && <Pin size={12} strokeWidth={1.75} className="pin" aria-label={t("read.pinned")} />}
                {m.answered && <Reply size={12} strokeWidth={1.75} aria-label={t("list.answered")} />}
                {m.has_attachments && <Paperclip size={12} strokeWidth={1.75} aria-label={t("list.attachment")} />}
                {m.flagged && <Flag size={12} strokeWidth={1.75} className="flag" aria-label={t("list.flagged")} />}
              </span>
              {account && <span className="account-dot row-account" data-color={account.color} role="img" aria-label={t("inbox.account", { name: account.name })} title={account.name} />}
              {due ? (
                <span className="row-date row-due">
                  <Clock size={11} strokeWidth={1.75} aria-hidden />
                  {due}
                </span>
              ) : (
                <span className="row-date">{listDate(m.date, today, lang)}</span>
              )}
            </span>
            <span className="row-subject">
              {m.important && <span className="important-badge">{t("list.important")}</span>}
              <bdi>{m.subject || t("list.noSubject")}</bdi>
            </span>
            {p.showSnippets && (
              <span className="row-snippet" dir="auto">
                {m.snippet || " "}
              </span>
            )}
          </span>
        </span>
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
    <section className="list-pane" aria-label={t("list.label")} data-folded={folded || undefined}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={p.onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{p.title}</h1>
          <span className="muted list-count">{p.searching ? t("list.results", { n: p.total }) : t("list.count", { n: p.total })}</span>
          {p.titleExtra}
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
        {p.head}
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
        onPointerDown={(e) => {
          if (e.pointerType === "touch" || e.button === 0) begin(e.clientX, e.clientY, e.target, e.pointerId);
        }}
        onPointerMove={(e) => {
          // Fingers move through touchmove (above), so a pull can hold the page still.
          if (e.pointerType !== "touch" && gesture.current?.pointer === e.pointerId) move(e.clientX, e.clientY);
        }}
        onPointerUp={(e) => end(e.clientX)}
        onPointerCancel={() => {
          if (gesture.current?.mode !== "pull") {
            gesture.current = null;
            setSwipe(null);
          }
        }}
        onTouchEnd={(e) => {
          const t0 = e.changedTouches[0];
          if (gesture.current && t0) end(t0.clientX);
        }}
      >
        {touch && (pull.dy > 0 || pull.busy) && (
          <div className="pull" style={{ height: pull.dy }} role={pull.busy ? "status" : undefined} aria-live="polite">
            {pull.busy ? <Spinner /> : <ArrowDown size={16} strokeWidth={1.75} aria-hidden className={pull.dy >= PULL_TRIGGER ? "pull-ready" : undefined} />}
            <span>{pull.busy ? t("list.refreshing") : pull.dy >= PULL_TRIGGER ? t("list.releaseToRefresh") : t("list.pullToRefresh")}</span>
          </div>
        )}
        {p.total === 0 && !p.loading ? (
          !p.searching && p.empty ? (
            p.empty
          ) : (
            <div className="empty">
              {p.searching ? <Search size={28} strokeWidth={1.5} aria-hidden /> : <Mail size={28} strokeWidth={1.5} aria-hidden />}
              <p>{p.emptyText}</p>
            </div>
          )
        ) : (
          <div className="messages-inner" style={{ height: Math.min(p.total * rowH, MAX_LIST_PX) }}>
            {rows}
          </div>
        )}
        {p.touch && p.total > 0 && <p className="list-end muted">{p.searching ? t("list.results", { n: p.total }) : t("list.count", { n: p.total })}</p>}
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
