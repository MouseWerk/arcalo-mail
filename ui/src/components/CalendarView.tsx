// The calendar, in Arcalo's design (its Kalender view): day, work week, week, month and a list;
// a time grid with an all-day row, appointments as quiet fills of their calendar's color, the
// „now“ line, and a side panel with the details. New appointments by dragging over a free
// stretch, a double click or N; appointments that can be changed are moved and resized with the
// mouse or the keyboard, renamed and deleted, each with „Rückgängig“. A meeting the user organizes
// changes only after „Aktualisierung an N Teilnehmende senden?“. Changes wait quietly for the
// server; a conflict offers „Server-Version übernehmen“ and „Meine Änderung behalten“.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import {
  AlertTriangle,
  CalendarClock,
  CalendarDays,
  CalendarPlus,
  ChevronLeft,
  ChevronRight,
  Eye,
  Lock,
  MapPin,
  Menu as MenuIcon,
  Plus,
  RefreshCw,
  Repeat,
  Settings2,
  Trash2,
  Undo2,
  User,
  Users,
  Video,
  X,
} from "lucide-react";
import { api, onCalendarChanged, openExternal } from "../lib/api";
import { useT, t as tr, type TKey } from "../lib/i18n";
import { now as clockNow } from "../lib/format";
import { setUndo, showToast } from "../lib/toasts";
import {
  attendeeName,
  blockFit,
  calendarColor,
  canEdit,
  dateLocale,
  durationMinutes,
  editedTimes,
  HOUR,
  hhmm,
  invited,
  isAllDayLike,
  isoDay,
  isoWeek,
  keyAction,
  layoutDay,
  minutesOfDay,
  monthCells,
  moved,
  onDay as coversDay,
  rangeTitle,
  resized,
  shownWeek,
  SNAP,
  step,
  timeRange,
  viewRange,
  weekLabel,
  weekStartsOn,
  dayEvents,
  dayText,
  parseDayText,
  parseTimeText,
  type CalView,
  type Range,
} from "../lib/calendar";
import type { CalendarRequest } from "../lib/calendar";
import type { CalendarEvent, CalendarInfo, CalendarOverview, CalFields, CalUndo, Settings } from "../lib/types";
import { ConfirmDialog, Segmented } from "./ui";

const VIEW_LABELS: Record<CalView, TKey> = { day: "calv.view.day", workweek: "calv.view.workweek", week: "calv.view.week", month: "calv.view.month", agenda: "calv.view.agenda" };
const BUSY_LABELS: Record<CalendarEvent["busy"], TKey> = { free: "calv.busy.free", tentative: "calv.busy.tentative", busy: "calv.busy.busy", oof: "calv.busy.oof", elsewhere: "calv.busy.elsewhere" };
const READ_ONLY: Partial<Record<CalendarEvent["editable"], TKey>> = { received: "entries.ro.received", series: "entries.ro.series", read_only: "entries.ro.readOnly", ics: "entries.ro.ics" };

function stored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key) as T | null;
    return v && allowed.includes(v) ? v : fallback;
  } catch {
    return fallback;
  }
}
function store(key: string, v: string) {
  try {
    localStorage.setItem(key, v);
  } catch {
    /* ignore */
  }
}

/** What the view showed last in this session (the view is unmounted when the place changes). */
const session: { anchor: Date | null; selected: string | null } = { anchor: null, selected: null };

/** A new appointment whose panel is open. */
type Draft = { start: Date; end: Date; title?: string; location?: string; description?: string; all_day?: boolean };

type Props = {
  settings: Settings;
  /** Settings → Kalender. */
  onSettings: () => void;
  /** Phones and tablets: the folders (the drawer). */
  onShowFolders: () => void;
  /** Open on an appointment or a day, or with a new appointment's draft (the assistant's), once. */
  request?: CalendarRequest | null;
  onRequestTaken?: () => void;
  /** A phone: one day, the list or the month; the weeks need a wider screen. */
  phone?: boolean;
};

function IconBtn({ icon: Icon, label, onClick, className, disabled, pressed }: { icon: typeof X; label: string; onClick: (e: React.MouseEvent<HTMLButtonElement>) => void; className?: string; disabled?: boolean; pressed?: boolean }) {
  return (
    <button type="button" className={`icon-btn${className ? ` ${className}` : ""}`} aria-label={label} title={label} onClick={onClick} disabled={disabled} aria-pressed={pressed}>
      <Icon size={16} strokeWidth={1.75} aria-hidden />
    </button>
  );
}

export function CalendarView({ settings, onSettings, onShowFolders, request, onRequestTaken, phone }: Props) {
  const t = useT();
  const cal = settings.calendar;
  const [chosen, setViewState] = useState<CalView>(() => stored("arcalo-mail.calendar.view", ["day", "workweek", "week", "month", "agenda"] as const, "workweek"));
  // On a phone a week shows as its day.
  const view: CalView = phone && (chosen === "week" || chosen === "workweek") ? "day" : chosen;
  const [anchor, setAnchorState] = useState(() => session.anchor ?? clockNow());
  const setAnchor = (next: Date | ((d: Date) => Date)) =>
    setAnchorState((d) => {
      const v = typeof next === "function" ? next(d) : next;
      session.anchor = v;
      return v;
    });
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [overview, setOverview] = useState<CalendarOverview | null>(null);
  const [selected, setSelectedState] = useState<string | null>(() => session.selected);
  const [draft, setDraft] = useState<Draft | null>(null);
  const setSelected = (k: string | null) => {
    session.selected = k;
    setSelectedState(k);
    if (k) setDraft(null);
  };
  const [tick, setTick] = useState(0);
  const [syncing, setSyncing] = useState(false);
  const [now, setNow] = useState(() => clockNow());
  const [ask, setAsk] = useState<{ n: number; resolve: (send: boolean | null) => void } | null>(null);
  const root = useRef<HTMLDivElement>(null);

  const startsOn = weekStartsOn();
  const workdays = cal.workdays;
  const range = useMemo(() => viewRange(view, anchor, startsOn, workdays, clockNow()), [view, anchor, startsOn, workdays.join(",")]);
  const rangeKey = `${view}:${isoDay(range.from)}:${isoDay(range.to)}`;
  const calendars = overview?.calendars ?? [];

  const setView = (v: CalView) => {
    setViewState(v);
    store("arcalo-mail.calendar.view", v);
  };

  // Only the latest request may fill the view (fast paging).
  const seq = useRef(0);
  useEffect(() => {
    const n = ++seq.current;
    api
      .calendarEvents(range.from.toISOString(), range.to.toISOString())
      .then((ev) => n === seq.current && setEvents(ev))
      .catch((e) => n === seq.current && showToast({ title: t("calv.loadFailed"), detail: String(e), tone: "danger" }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeKey, tick]);

  useEffect(() => {
    const load = () => void api.calendarOverview().then(setOverview).catch(() => {});
    load();
    return onCalendarChanged(() => {
      load();
      setTick((x) => x + 1);
    });
  }, []);

  // The „now“ line moves every minute.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(clockNow()), 60_000);
    return () => clearInterval(timer);
  }, []);

  // Asked for from elsewhere: an invitation's appointment or day, the assistant's draft.
  useEffect(() => {
    if (!request) return;
    if (request.draft) {
      const d = request.draft;
      const start = new Date(d.start);
      setAnchor(start);
      if (view === "month" || view === "agenda" || (view === "workweek" && !cal.workdays.includes(((start.getDay() + 6) % 7) + 1))) setViewState("week");
      setSelected(null);
      setDraft({ start, end: new Date(d.end), title: d.title, location: d.location, description: d.description, all_day: d.all_day });
    } else {
      if (request.day) {
        const day = new Date(request.day);
        setAnchor(day);
        // A day outside the work week: the whole week for it (not remembered).
        if (view === "workweek" && !cal.workdays.includes(((day.getDay() + 6) % 7) + 1)) setViewState("week");
        if (view === "month" || view === "agenda") setViewState("week");
      }
      setSelected(request.key ?? null);
    }
    onRequestTaken?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request]);

  const current = events.find((e) => e.key === selected) ?? null;

  // Changes here: shown at once, then as the backend has them.
  const patchEvent = (key: string, e: CalendarEvent | null) =>
    setEvents((list) => (e ? (list.some((x) => x.key === key) ? list.map((x) => (x.key === key ? e : x)) : [...list, e]) : list.filter((x) => x.key !== key)));

  /** Asks before the attendees of an organized meeting get the change; `null`: cancelled. */
  const confirmSend = (e: CalendarEvent): Promise<boolean | null> => {
    if (e.editable !== "confirm") return Promise.resolve(false);
    return new Promise((resolve) => setAsk({ n: invited(e), resolve }));
  };

  const undoChange = (u: CalUndo) =>
    void api
      .calendarUndo(u)
      .then(() => {
        setTick((x) => x + 1);
        showToast({ title: t("toast.undone"), duration: 2500 });
      })
      .catch((err) => showToast({ title: t("entries.undoFailed"), detail: String(err), tone: "danger" }));

  const offerUndo = (title: string, u: CalUndo) => {
    setUndo(() => undoChange(u));
    showToast({ title, duration: 5000, action: { label: t("common.undo"), run: () => undoChange(u) } });
  };

  const saveEntry = async (e: CalendarEvent, patch: CalFields) => {
    const send = await confirmSend(e);
    if (send === null) {
      patchEvent(e.key, e);
      return;
    }
    patchEvent(e.key, { ...e, ...patch, write: e.write ?? { op: "update", state: "pending", error: null, server: null, mine: null, send } } as CalendarEvent);
    try {
      const r = await api.calendarUpdate(e.key, patch, send);
      if (r.event) patchEvent(e.key, r.event);
      const what = patch.start || patch.end ? (patch.title ? "entries.changed" : "entries.moved") : "entries.changed";
      offerUndo(t(what as TKey, { title: r.event?.title ?? e.title }), r.undo);
    } catch (err) {
      patchEvent(e.key, e);
      showToast({ title: t("entries.saveFailed"), detail: String(err), tone: "danger" });
    }
  };

  // Only appointments without attendees are deleted here (a meeting is cancelled where it was made).
  const deleteEntry = async (e: CalendarEvent) => {
    if (e.editable !== "yes") return;
    if (selected === e.key) setSelected(null);
    patchEvent(e.key, null);
    try {
      const u = await api.calendarDelete(e.key);
      offerUndo(t("entries.deleted", { title: e.title }), u);
    } catch (err) {
      patchEvent(e.key, e);
      showToast({ title: t("entries.deleteFailed"), detail: String(err), tone: "danger" });
    }
  };

  const createOn = true;
  const newEntry = (start: Date, end: Date) => {
    setSelected(null);
    setDraft({ start, end });
  };
  const newEntryAt = (day: Date, minute: number) => {
    const len = cal.entry_minutes || 30;
    const m = Math.max(0, Math.min(minute, 24 * 60 - len));
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, m);
    newEntry(start, new Date(start.getTime() + len * 60_000));
  };
  // N or the button: today at the next half hour, another day at 9:00.
  const newEntryNow = () => {
    const d = clockNow();
    const day = range.days.find((x) => isoDay(x) === isoDay(d)) ?? (view === "month" || view === "agenda" ? d : anchor);
    const minute = isoDay(day) === isoDay(d) ? Math.ceil((minutesOfDay(d) + 1) / 30) * 30 : 9 * 60;
    if (view === "month" || view === "agenda") setView(view === "month" ? "week" : "workweek");
    if (isoDay(day) !== isoDay(anchor)) setAnchor(day);
    newEntryAt(day, Math.min(minute, 23 * 60));
  };

  const go = (dir: -1 | 1) => setAnchor((a) => step(view, a, dir));
  const today = () => setAnchor(clockNow());
  const openDay = (d: Date) => {
    setAnchor(d);
    setView("day");
  };

  // Keys work while nothing else (a field, a dialog, a menu) has them.
  const keyState = useRef({ selected, go, today, setView, draft, newEntryNow });
  keyState.current = { selected, go, today, setView, draft, newEntryNow };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing || e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      const focused = document.activeElement as HTMLElement | null;
      if (focused?.closest("input, textarea, select, [contenteditable='true'], .calv-appt")) return;
      if (document.querySelector(".overlay, .menu")) return;
      const k = keyState.current;
      if (e.key === "Escape") {
        if (k.selected || k.draft) {
          e.preventDefault();
          setSelected(null);
          setDraft(null);
        }
        return;
      }
      if ((e.key === "n" || e.key === "N") && !e.shiftKey) {
        e.preventDefault();
        k.newEntryNow();
        return;
      }
      const a = keyAction(e.key);
      if (!a) return;
      e.preventDefault();
      if (a.move) k.go(a.move);
      if (a.today) k.today();
      if (a.view) k.setView(a.view);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const sync = async () => {
    setSyncing(true);
    try {
      const sources = await api.calendarSync();
      setOverview((o) => (o ? { ...o, sources } : o));
      setTick((x) => x + 1);
      const failed = sources.filter((x) => x.error);
      if (failed.length) showToast({ title: t("calv.notAllSynced"), detail: failed.map((f) => `${f.name}: ${f.error}`).join("\n"), tone: "danger" });
    } catch (e) {
      showToast({ title: t("calv.syncFailed"), detail: String(e), tone: "danger" });
    } finally {
      setSyncing(false);
    }
  };

  const toggleCalendar = (c: CalendarInfo) =>
    void api
      .calendarSet(c.id, { hidden: !c.hidden })
      .then((next) => {
        setOverview((o) => (o ? { ...o, calendars: o.calendars.map((x) => (x.id === next.id ? next : x)) } : o));
        setTick((x) => x + 1);
      })
      .catch((e) => showToast({ title: t("common.notSaved"), detail: String(e), tone: "danger" }));

  const errors = overview?.sources.filter((x) => x.error) ?? [];
  // A phone names the month over a day, as the day's own head shows its date.
  const title = rangeTitle(phone && view === "day" ? "month" : view, range, anchor);
  const viewOptions = ((phone ? ["day", "agenda", "month"] : ["day", "workweek", "week", "month", "agenda"]) as CalView[]).map((v) => [v, t(VIEW_LABELS[v])] as [CalView, string]);
  const writable = calendars.filter((c) => c.writable && c.kind !== "ics" && !c.hidden);

  return (
    <section className={`calv ${current || draft ? "with-detail" : ""}`} ref={root} tabIndex={-1} aria-label={t("nav.calendar")}>
      <header className="calv-head">
        <div className="calv-heading">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{title}</h1>
          {view !== "month" && view !== "agenda" && <span className="calv-kw">{weekLabel(range)}</span>}
          {view === "month" && <span className="calv-kw">{t("cal.weekNo", { n: `${shownWeek(range.days[0])}–${isoWeek(range.days[range.days.length - 1])}` })}</span>}
        </div>
        <div className="calv-tools">
          <div className="calv-nav" role="group" aria-label={t("calv.navigate")}>
            <IconBtn icon={ChevronLeft} label={t("calv.back")} onClick={() => go(-1)} />
            <button type="button" className="btn btn-sm btn-quiet" onClick={today} title={t("calv.todayKey")}>
              {t("calv.today")}
            </button>
            <IconBtn icon={ChevronRight} label={t("calv.forward")} onClick={() => go(1)} />
          </div>
          <div className="calv-views">
            <Segmented label={t("calv.view")} value={view} onChange={setView} options={viewOptions} />
          </div>
          <select className="select select-sm calv-view-select" aria-label={t("calv.view")} value={view} onChange={(e) => setView(e.target.value as CalView)}>
            {viewOptions.map(([v, label]) => (
              <option key={v} value={v}>
                {label}
              </option>
            ))}
          </select>
          <div className="calv-actions">
            <button type="button" className="btn btn-sm calv-new" onClick={newEntryNow} title={t("calv.newKey")}>
              <Plus size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("entries.newButton")}</span>
            </button>
            <IconBtn icon={RefreshCw} label={t("calv.syncNow")} className={syncing ? "spinning" : ""} onClick={() => void sync()} />
            <IconBtn icon={Settings2} label={t("calv.settings")} onClick={onSettings} />
          </div>
        </div>
      </header>

      {errors.length > 0 && (
        <div className="calv-banner" role="status">
          <AlertTriangle size={14} strokeWidth={1.75} aria-hidden />
          <span>
            {errors.map((x) => (
              <span key={x.id} className="calv-banner-item">
                <b>{x.name}:</b> {x.error}
              </span>
            ))}
          </span>
          <button type="button" className="btn btn-sm btn-quiet" onClick={onSettings}>
            {t("calv.settings")}
          </button>
        </div>
      )}

      <CalendarLegend calendars={calendars} onToggle={toggleCalendar} />
      <div className="calv-main">
        <div className="calv-body">
          {view === "month" ? (
            <MonthGrid range={range} anchor={anchor} events={events} calendars={calendars} selected={selected} onSelect={setSelected} onDay={openDay} now={now} />
          ) : view === "agenda" ? (
            <AgendaList range={range} events={events} calendars={calendars} selected={selected} onSelect={setSelected} now={now} />
          ) : (
            <TimeGrid
              range={range}
              rangeKey={rangeKey}
              events={events}
              calendars={calendars}
              now={now}
              selected={selected}
              onSelect={setSelected}
              onDay={openDay}
              createOn={createOn}
              draft={draft}
              onCreateRange={newEntry}
              onCreateAt={newEntryAt}
              onEntryChange={(e, p) => void saveEntry(e, p)}
              onEntryDelete={(e) => void deleteEntry(e)}
            />
          )}
        </div>
        {draft && !current && (
          <EntryEditor
            draft={draft}
            calendars={writable}
            defaultCalendar={overview?.default ?? null}
            onChange={setDraft}
            onClose={() => setDraft(null)}
            onCreated={(e, u) => {
              setDraft(null);
              if (e) {
                patchEvent(e.key, e);
                setSelected(e.key);
              }
              offerUndo(t("entries.created", { title: e?.title ?? "" }), u);
              void api.calendarOverview().then(setOverview);
            }}
          />
        )}
        {current && (
          <EventDetail
            event={current}
            calendars={calendars}
            onClose={() => setSelected(null)}
            onSave={(p) => void saveEntry(current, p)}
            onDelete={() => void deleteEntry(current)}
            onReload={() => setTick((x) => x + 1)}
          />
        )}
      </div>
      {ask && (
        <ConfirmDialog
          title={ask.n > 0 ? t("entries.sendTitle", { n: ask.n }) : t("entries.sendTitleAll")}
          text={t("entries.sendText")}
          confirm={t("entries.send")}
          onConfirm={() => ask.resolve(true)}
          onClose={() => {
            // Closed without sending: nothing changes (onConfirm resolved first otherwise).
            ask.resolve(null);
            setAsk(null);
          }}
        />
      )}
    </section>
  );
}

// ---------------------------------------------------------------- legend

/** The calendars with their colors; a click hides or shows one (it keeps syncing). */
function CalendarLegend({ calendars, onToggle }: { calendars: CalendarInfo[]; onToggle: (c: CalendarInfo) => void }) {
  const t = useT();
  if (calendars.length < 2) return null;
  return (
    <div className="calv-legend" role="group" aria-label={t("calv.legend")}>
      {calendars.map((c) => {
        const on = !c.hidden;
        return (
          <button
            type="button"
            key={c.id}
            className={`calv-legend-item ${on ? "" : "off"}`}
            style={{ "--ev": c.color } as CSSProperties}
            aria-pressed={on}
            title={on ? t("calv.hide", { name: c.name }) : t("calv.show", { name: c.name })}
            onClick={() => onToggle(c)}
          >
            <span className="calv-legend-dot" aria-hidden />
            <span className="ellipsis">{c.name}</span>
          </button>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- time grid

function eventStyle(e: CalendarEvent, calendars: CalendarInfo[]): CSSProperties {
  return { "--ev": calendarColor(e.calendar_id, calendars) } as CSSProperties;
}

/** The states of a block: busy status, selected, over, running, private, and a waiting change. */
function eventClass(e: CalendarEvent, selected: string | null, now: Date) {
  const n = now.getTime();
  const past = !e.all_day && new Date(e.end).getTime() <= n;
  const live = !e.all_day && !past && new Date(e.start).getTime() <= n;
  return ["calv-ev", `busy-${e.busy}`, e.key === selected ? "selected" : "", past ? "past" : "", live ? "live" : "", e.private ? "veiled" : "", e.write ? `write-${e.write.state}` : ""].filter(Boolean).join(" ");
}

function EventMarks({ e }: { e: CalendarEvent }) {
  const t = useT();
  return (
    <>
      {e.link && <Video className="calv-mark" size={11} aria-label={t("calv.online")} />}
      {e.private && <Lock className="calv-mark" size={10} aria-label={t("calv.private")} />}
      {e.recurring && <Repeat className="calv-mark faint-mark" size={10} aria-label={t("calv.series")} />}
      {e.write?.state === "pending" && <CalendarClock className="calv-mark" size={11} aria-label={t("entries.pendingShort")} />}
      {(e.write?.state === "conflict" || e.write?.state === "refused") && <AlertTriangle className="calv-mark conflict" size={11} aria-label={t("entries.conflict")} />}
    </>
  );
}

const startTime = (e: CalendarEvent) => timeRange(e).slice(0, 5);

/** „Montag, 5. Oktober“. */
function dayLabel(d: Date, l = dateLocale()) {
  return d.toLocaleDateString(l, { weekday: "long", day: "numeric", month: "long" });
}

function evLabel(e: CalendarEvent) {
  return [e.title, timeRange(e), e.location].filter(Boolean).join(", ");
}

function DayHead({ d, onDay, now }: { d: Date; onDay: (d: Date) => void; now: Date }) {
  const t = useT();
  const iso = isoDay(d);
  const l = dateLocale();
  return (
    <div className={`calv-dayhead ${iso === isoDay(now) ? "today" : ""}`} data-date={iso}>
      <button type="button" className="calv-dayhead-date" onClick={() => onDay(d)} aria-label={t("calv.showDay", { day: dayLabel(d, l) })}>
        <span className="calv-wd">{d.toLocaleDateString(l, { weekday: "short" }).replace(".", "")}</span>
        <span className="calv-dn">{d.getDate()}</span>
      </button>
    </div>
  );
}

type GridProps = {
  range: Range;
  rangeKey: string;
  events: CalendarEvent[];
  calendars: CalendarInfo[];
  now: Date;
  selected: string | null;
  onSelect: (k: string) => void;
  onDay: (d: Date) => void;
  createOn: boolean;
  draft: Draft | null;
  onCreateRange: (start: Date, end: Date) => void;
  onCreateAt: (day: Date, minute: number) => void;
  onEntryChange: (e: CalendarEvent, p: CalFields) => void;
  onEntryDelete: (e: CalendarEvent) => void;
};

function TimeGrid(props: GridProps) {
  const t = useT();
  const { range, events, calendars, now, selected, onSelect, onDay } = props;
  // Dragging over a free stretch of a day: from and to (minutes of that day).
  const [sweep, setSweep] = useState<{ iso: string; day: Date; from: number; to: number } | null>(null);
  const free = (e: { target: EventTarget; currentTarget: EventTarget }) => {
    const el = e.target as HTMLElement;
    return el === e.currentTarget || el.classList.contains("calv-meetings");
  };
  const sweepRange = (sw: { day: Date; from: number; to: number }) => {
    const lo = Math.min(sw.from, sw.to);
    const hi = Math.max(sw.from, sw.to) + SNAP;
    const d = sw.day;
    return { start: new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, lo), end: new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, Math.min(hi, 24 * 60)) };
  };
  const scroll = useRef<HTMLDivElement>(null);
  const scrolledFor = useRef("");
  const allDay = events.filter(isAllDayLike);
  const timed = events.filter((e) => !isAllDayLike(e));

  // Opens on the working hours (or the first meeting, when earlier).
  useLayoutEffect(() => {
    const el = scroll.current;
    if (!el || scrolledFor.current === props.rangeKey) return;
    scrolledFor.current = props.rangeKey;
    const first = timed.filter((e) => range.days.some((d) => coversDay(e, d))).map((e) => minutesOfDay(new Date(e.start)));
    const start = Math.min(7 * 60 + 30, ...first.map((m) => m - 30));
    el.scrollTop = Math.max(0, (start / 60) * HOUR);
  });

  const at = (e: { clientY: number; currentTarget: HTMLElement }) => Math.max(0, Math.min(24 * 60 - SNAP, Math.floor(((e.clientY - e.currentTarget.getBoundingClientRect().top) / HOUR) * 4) * SNAP));
  const todayIso = isoDay(now);
  return (
    <div className={`calv-grid days-${range.days.length}`} style={{ "--days": range.days.length } as CSSProperties}>
      <div className="calv-scroll" ref={scroll}>
        <div className="calv-sticky">
          <div className="calv-row calv-heads">
            <div className="calv-gutter" />
            {range.days.map((d) => (
              <DayHead key={isoDay(d)} d={d} onDay={onDay} now={now} />
            ))}
          </div>
          <div className="calv-row calv-allday">
            <div className="calv-gutter calv-allday-label">{t("cal.allDay")}</div>
            {range.days.map((d) => (
              <div key={isoDay(d)} className="calv-allday-cell">
                {allDay
                  .filter((e) => coversDay(e, d))
                  .map((e) => (
                    <button
                      type="button"
                      key={e.key}
                      className={`${eventClass(e, selected, now)} calv-chip`}
                      style={eventStyle(e, calendars)}
                      onClick={() => onSelect(e.key)}
                      aria-label={evLabel(e)}
                      aria-expanded={selected === e.key}
                      data-key={e.key}
                      title={e.title}
                    >
                      <span className="calv-ev-title">{e.title}</span>
                      <span className="calv-ev-marks">
                        <EventMarks e={e} />
                      </span>
                    </button>
                  ))}
              </div>
            ))}
          </div>
        </div>
        <div className="calv-row calv-timeline" style={{ height: 24 * HOUR }}>
          <div className="calv-gutter calv-hours" aria-hidden>
            {Array.from({ length: 24 }, (_, h) => (
              <span key={h} style={{ top: h * HOUR }}>
                {h === 0 ? "" : `${String(h).padStart(2, "0")}:00`}
              </span>
            ))}
          </div>
          {range.days.map((d, dayIndex) => {
            const iso = isoDay(d);
            const placed = layoutDay(timed, d);
            const weekend = [0, 6].includes(d.getDay());
            return (
              <div
                key={iso}
                className={`calv-col ${weekend ? "weekend" : ""} ${iso === todayIso ? "today" : ""}`}
                role="group"
                aria-label={dayLabel(d)}
                data-date={iso}
                onPointerDown={(e) => {
                  if (!props.createOn || e.button !== 0 || !free(e)) return;
                  const m = at(e);
                  e.currentTarget.setPointerCapture?.(e.pointerId);
                  setSweep({ iso, day: d, from: m, to: m });
                }}
                onPointerMove={(e) => {
                  if (sweep?.iso !== iso) return;
                  const m = at(e);
                  if (m !== sweep.to) setSweep({ ...sweep, to: m });
                }}
                onPointerUp={(e) => {
                  if (sweep?.iso !== iso) return;
                  e.currentTarget.releasePointerCapture?.(e.pointerId);
                  setSweep(null);
                  // A click alone makes nothing (a double click does).
                  if (sweep.to === sweep.from) return;
                  const r = sweepRange(sweep);
                  props.onCreateRange(r.start, r.end);
                }}
                onPointerCancel={() => setSweep(null)}
                onDoubleClick={(e) => {
                  if (!props.createOn || !free(e)) return;
                  const m = Math.floor(((e.clientY - e.currentTarget.getBoundingClientRect().top) / HOUR) * 2) * 30;
                  props.onCreateAt(d, m);
                }}
              >
                <div className="calv-meetings">
                  {placed.map((p) => {
                    const e = p.item;
                    const height = Math.max((p.height / 60) * HOUR - 2, 16);
                    const style = {
                      ...eventStyle(e, calendars),
                      top: (p.top / 60) * HOUR,
                      height,
                      left: `calc((100% + 2px) * ${p.col / p.cols})`,
                      width: `calc((100% + 2px) / ${p.cols} - 2px)`,
                    } as CSSProperties;
                    const fit = blockFit(height, p.height, !!e.location);
                    const body = (shown: { start: string; end: string }) => {
                      const when = timeRange({ ...e, ...shown });
                      return (
                        <>
                          <span className="calv-ev-title">{e.title}</span>
                          <span className="calv-ev-meta calv-ev-time">
                            <span className="ellipsis">{fit.oneLine ? when.slice(0, 5) : fit.place || !e.location ? when : `${when} · ${e.location}`}</span>
                            <span className="calv-ev-marks">
                              <EventMarks e={e} />
                            </span>
                          </span>
                          {fit.place && <span className="calv-ev-meta calv-ev-place">{e.location}</span>}
                        </>
                      );
                    };
                    if (canEdit(e)) {
                      return (
                        <EntryItem
                          key={e.key}
                          event={e}
                          className={`${eventClass(e, selected, now)} ${fit.oneLine ? "short" : ""}`}
                          style={{ ...style, "--lines": fit.titleLines } as CSSProperties}
                          dayIndex={dayIndex}
                          dayCount={range.days.length}
                          label={t("entries.label", { title: e.title, time: timeRange(e) })}
                          tooltip={`${e.title} · ${timeRange(e)}`}
                          selected={selected === e.key}
                          onSelect={onSelect}
                          onChange={props.onEntryChange}
                          onDelete={props.onEntryDelete}
                        >
                          {body}
                        </EntryItem>
                      );
                    }
                    return (
                      <button
                        type="button"
                        key={e.key}
                        className={`${eventClass(e, selected, now)} ${fit.oneLine ? "short" : ""}`}
                        style={{ ...style, "--lines": fit.titleLines } as CSSProperties}
                        onClick={() => onSelect(e.key)}
                        aria-label={evLabel(e)}
                        aria-expanded={selected === e.key}
                        title={`${e.title} · ${timeRange(e)}`}
                        data-key={e.key}
                      >
                        {body({ start: e.start, end: e.end })}
                      </button>
                    );
                  })}
                </div>
                {sweep?.iso === iso && sweep.to !== sweep.from && <EntryGhost {...sweepRange(sweep)} />}
                {!sweep && props.draft && !props.draft.all_day && isoDay(props.draft.start) === iso && <EntryGhost start={props.draft.start} end={props.draft.end} title={props.draft.title} />}
                {iso === todayIso && <div className="calv-now" style={{ top: (minutesOfDay(now) / 60) * HOUR }} aria-hidden />}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

interface Drag {
  mode: "move" | "resize";
  x: number;
  y: number;
  colWidth: number;
  minutes: number;
  days: number;
  active: boolean;
}

/**
 * An appointment that can be changed, in the time grid: a click opens it, dragging moves it (also
 * to another day), the handle at its foot changes its length; the arrow keys do the same (with
 * Umschalt the length), Entf deletes it.
 */
function EntryItem(props: {
  event: CalendarEvent;
  className: string;
  style: CSSProperties;
  dayIndex: number;
  dayCount: number;
  label: string;
  tooltip: string;
  selected: boolean;
  onSelect: (key: string) => void;
  onChange: (e: CalendarEvent, p: { start: string; end: string }) => void;
  onDelete: (e: CalendarEvent) => void;
  children: (shown: { start: string; end: string }) => ReactNode;
}) {
  const t = useT();
  const { event: e } = props;
  const [drag, setDrag] = useState<Drag | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  // A drag ends with a click on the same element: that click must not open the appointment.
  const dragged = useRef(false);

  const down = (ev: ReactPointerEvent<HTMLDivElement>) => {
    if (ev.button !== 0) return;
    const resize = (ev.target as HTMLElement).closest(".calv-entry-handle") != null;
    const col = ref.current?.closest(".calv-col");
    ev.stopPropagation();
    dragged.current = false;
    ref.current?.setPointerCapture?.(ev.pointerId);
    setDrag({ mode: resize ? "resize" : "move", x: ev.clientX, y: ev.clientY, colWidth: col?.getBoundingClientRect().width ?? 1, minutes: 0, days: 0, active: false });
  };
  const move = (ev: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    const dy = ev.clientY - drag.y;
    const dx = ev.clientX - drag.x;
    const active = drag.active || Math.abs(dy) > 3 || Math.abs(dx) > 3;
    const minutes = Math.round(((dy / HOUR) * 60) / SNAP) * SNAP;
    const days = drag.mode === "move" ? Math.max(-props.dayIndex, Math.min(props.dayCount - 1 - props.dayIndex, Math.round(dx / drag.colWidth))) : 0;
    if (active !== drag.active || minutes !== drag.minutes || days !== drag.days) setDrag({ ...drag, active, minutes, days });
  };
  const up = (ev: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    ref.current?.releasePointerCapture?.(ev.pointerId);
    setDrag(null);
    if (!drag.active) return;
    dragged.current = true;
    if (!drag.minutes && !drag.days) return;
    props.onChange(e, drag.mode === "move" ? moved(e, drag.minutes, drag.days) : resized(e, drag.minutes));
  };
  const click = () => {
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    props.onSelect(e.key);
  };
  const key = (ev: ReactKeyboardEvent<HTMLDivElement>) => {
    if (ev.nativeEvent.isComposing || ev.ctrlKey || ev.metaKey || ev.altKey) return;
    let p: { start: string; end: string } | null = null;
    if (ev.key === "ArrowUp" || ev.key === "ArrowDown") {
      const d = ev.key === "ArrowUp" ? -SNAP : SNAP;
      p = ev.shiftKey ? resized(e, d) : moved(e, d);
    } else if (ev.key === "ArrowLeft" || ev.key === "ArrowRight") {
      const d = ev.key === "ArrowLeft" ? -1 : 1;
      if (props.dayIndex + d < 0 || props.dayIndex + d >= props.dayCount) return;
      p = moved(e, 0, d);
    } else if (ev.key === "Enter" || ev.key === " " || ev.key === "F2") {
      ev.preventDefault();
      props.onSelect(e.key);
      return;
    } else if ((ev.key === "Delete" || ev.key === "Backspace") && e.editable === "yes") {
      ev.preventDefault();
      props.onDelete(e);
      return;
    }
    if (!p) return;
    ev.preventDefault();
    ev.stopPropagation();
    props.onChange(e, p);
  };

  const shift = drag?.active ? drag : null;
  const top = Number(props.style.top ?? 0) + (shift?.mode === "move" ? (shift.minutes / 60) * HOUR : 0);
  const height = Math.max(HOUR / 4 - 2, Number(props.style.height ?? 0) + (shift?.mode === "resize" ? (shift.minutes / 60) * HOUR : 0));
  const shown = shift ? (shift.mode === "move" ? moved(e, shift.minutes, shift.days) : resized(e, shift.minutes)) : { start: e.start, end: e.end };
  return (
    <div
      ref={ref}
      role="button"
      tabIndex={0}
      data-key={e.key}
      className={`${props.className} calv-appt ${drag?.active ? "dragging" : ""}`}
      style={{ ...props.style, top, height, transform: shift?.days ? `translateX(${shift.days * shift.colWidth}px)` : undefined }}
      aria-label={props.label}
      aria-description={t("entries.keys")}
      aria-expanded={props.selected}
      title={drag ? undefined : props.tooltip}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={() => setDrag(null)}
      onClick={click}
      onKeyDown={key}
    >
      {props.children(shown)}
      <span className="calv-entry-handle" aria-hidden title={t("entries.resize")} />
    </div>
  );
}

/** The stretch of a new appointment in the grid (while dragging, and while its panel is open). */
function EntryGhost({ start, end, title }: { start: Date; end: Date; title?: string }) {
  const t = useT();
  const day = new Date(start.getFullYear(), start.getMonth(), start.getDate()).getTime();
  const top = ((start.getTime() - day) / 3_600_000) * HOUR;
  const height = Math.max(12, ((end.getTime() - start.getTime()) / 3_600_000) * HOUR - 2);
  return (
    <div className="calv-entry-ghost" style={{ top, height }} aria-hidden>
      <span className="ellipsis">{title?.trim() || t("entries.new")}</span>
      <span className="num">{`${hhmm(start)}–${hhmm(end)}`}</span>
    </div>
  );
}

// ---------------------------------------------------------------- month

function MonthGrid(props: { range: Range; anchor: Date; events: CalendarEvent[]; calendars: CalendarInfo[]; selected: string | null; onSelect: (k: string) => void; onDay: (d: Date) => void; now: Date }) {
  const t = useT();
  const { range, anchor, events, calendars, selected, onSelect, onDay, now } = props;
  const cells = useMemo(() => monthCells(events, range.days, 4), [events, range]);
  const weeks = Array.from({ length: range.days.length / 7 }, (_, w) => range.days.slice(w * 7, w * 7 + 7));
  const l = dateLocale();
  const todayIso = isoDay(now);
  return (
    <div className="calv-month" role="grid" aria-label={t("calv.view.month")}>
      <div className="calv-month-row calv-month-head" role="row">
        <div className="calv-month-kw">{t("calv.weekShort")}</div>
        {weeks[0].map((d) => (
          <div key={d.getDay()} className="calv-month-wd" role="columnheader">
            {d.toLocaleDateString(l, { weekday: "short" }).replace(".", "")}
          </div>
        ))}
      </div>
      {weeks.map((week) => (
        <div key={isoDay(week[0])} className="calv-month-row" role="row">
          <div className="calv-month-kw">{shownWeek(week[0])}</div>
          {week.map((d) => {
            const iso = isoDay(d);
            const cell = cells.get(iso);
            const out = d.getMonth() !== anchor.getMonth();
            return (
              <div key={iso} className={`calv-mcell ${out ? "out" : ""} ${iso === todayIso ? "today" : ""}`} role="gridcell" data-date={iso}>
                <div className="calv-mcell-head">
                  <button type="button" className="calv-mday" onClick={() => onDay(d)} aria-label={t("calv.showDay", { day: dayLabel(d, l) })}>
                    {d.getDate()}
                  </button>
                </div>
                <div className="calv-mlist">
                  {cell?.shown.map((e) => (
                    <button
                      type="button"
                      key={e.key}
                      className={`${eventClass(e, selected, now)} calv-mev ${isAllDayLike(e) ? "allday" : ""}`}
                      style={eventStyle(e, calendars)}
                      onClick={() => onSelect(e.key)}
                      aria-label={evLabel(e)}
                      aria-expanded={selected === e.key}
                      data-key={e.key}
                      title={`${e.title} · ${timeRange(e)}`}
                    >
                      {!isAllDayLike(e) && <span className="calv-mev-time">{startTime(e)}</span>}
                      <span className="calv-ev-title">{e.title}</span>
                      <span className="calv-ev-marks">
                        <EventMarks e={e} />
                      </span>
                    </button>
                  ))}
                  {!!cell?.more && (
                    <button type="button" className="calv-more" onClick={() => onDay(d)}>
                      {t("calv.more", { n: cell.more })}
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- list

function AgendaList({ range, events, calendars, selected, onSelect, now }: { range: Range; events: CalendarEvent[]; calendars: CalendarInfo[]; selected: string | null; onSelect: (k: string) => void; now: Date }) {
  const t = useT();
  const l = dateLocale();
  const days = range.days.map((d) => ({ d, list: dayEvents(events, d) })).filter((x) => x.list.length);
  if (!days.length)
    return (
      <div className="calv-empty">
        <div className="empty">
          <CalendarDays size={28} strokeWidth={1.5} aria-hidden />
          <p className="empty-title">{t("calv.noEvents")}</p>
          <p className="muted">{t("calv.noEventsText", { n: range.days.length })}</p>
        </div>
      </div>
    );
  const todayIso = isoDay(now);
  return (
    <div className="calv-agenda">
      {days.map(({ d, list }) => (
        <section key={isoDay(d)} className={`calv-agenda-day ${isoDay(d) === todayIso ? "today" : ""}`}>
          <h2>
            <span>{d.toLocaleDateString(l, { weekday: "long" })}</span>
            <span className="muted">{d.toLocaleDateString(l, { day: "numeric", month: "long" })}</span>
          </h2>
          {list.map((e) => (
            <button
              type="button"
              key={e.key}
              className={`calv-agenda-row ${e.key === selected ? "selected" : ""} ${!e.all_day && new Date(e.end) <= now ? "past" : ""}`}
              style={eventStyle(e, calendars)}
              onClick={() => onSelect(e.key)}
              aria-label={evLabel(e)}
              aria-expanded={selected === e.key}
              data-key={e.key}
            >
              <span className="calv-agenda-time">{timeRange(e)}</span>
              <span className="calv-agenda-dot" aria-hidden />
              <span className="calv-agenda-text">
                <span className="calv-ev-title">{e.title}</span>
                {e.location && <span className="calv-ev-meta">{e.location}</span>}
              </span>
              <span className="calv-agenda-marks">{e.link && <Video size={13} strokeWidth={1.75} className="muted" aria-label={t("calv.online")} />}</span>
            </button>
          ))}
        </section>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- panels

/**
 * Date, start and end in the app's formats; `onCommit` gets the new times when the focus leaves
 * them or Eingabe is pressed in them, once per change (a new start keeps the length).
 */
function WhenInputs({ start, end, onCommit }: { start: Date; end: Date; onCommit: (s: Date, e: Date) => void }) {
  const t = useT();
  const [date, setDate] = useState(dayText(start));
  const [from, setFrom] = useState(hhmm(start));
  const [to, setTo] = useState(hhmm(end));
  const sent = useRef("");
  useEffect(() => {
    setDate(dayText(start));
    setFrom(hhmm(start));
    setTo(hhmm(end));
    sent.current = "";
  }, [start.getTime(), end.getTime()]);
  const commit = () => {
    const day = parseDayText(date);
    const f = parseTimeText(from);
    const e = parseTimeText(to);
    if (!day || !f || !e) {
      // Not a day or a time: the fields show what is saved again.
      setDate(dayText(start));
      setFrom(hhmm(start));
      setTo(hhmm(end));
      return;
    }
    const r = editedTimes(start, end, { date: day, from: f, to: e });
    if (!r) return;
    const k = `${r.start.getTime()}-${r.end.getTime()}`;
    if (k === sent.current) return;
    sent.current = k;
    onCommit(r.start, r.end);
  };
  return (
    <div
      className="calv-entry-when"
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.nativeEvent.isComposing) commit();
      }}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) commit();
      }}
    >
      <input className="input date-input" inputMode="numeric" spellCheck={false} aria-label={t("entries.date")} value={date} onChange={(e) => setDate(e.target.value)} />
      <input className="input time-input" inputMode="numeric" spellCheck={false} aria-label={t("entries.start")} value={from} onChange={(e) => setFrom(e.target.value)} />
      <span className="muted" aria-hidden>
        –
      </span>
      <input className="input time-input" inputMode="numeric" spellCheck={false} aria-label={t("entries.end")} value={to} onChange={(e) => setTo(e.target.value)} />
    </div>
  );
}

/** The panel of a new appointment: title, date and times, place, calendar. */
function EntryEditor({ draft, calendars, defaultCalendar, onChange, onClose, onCreated }: { draft: Draft; calendars: CalendarInfo[]; defaultCalendar: number | null; onChange: (d: Draft) => void; onClose: () => void; onCreated: (e: CalendarEvent | null, u: CalUndo) => void }) {
  const t = useT();
  const [title, setTitle] = useState(draft.title ?? "");
  const [location, setLocation] = useState(draft.location ?? "");
  const [target, setTarget] = useState<number | null>(() => (calendars.some((c) => c.id === defaultCalendar) ? defaultCalendar : (calendars[0]?.id ?? null)));
  const [busy, setBusy] = useState(false);
  const when = useRef(draft);
  useEffect(() => {
    when.current = draft;
  }, [draft.start.getTime(), draft.end.getTime()]);
  const create = async () => {
    if (busy) return;
    setBusy(true);
    const { start, end } = when.current;
    try {
      const r = await api.calendarCreate(target, {
        title: title.trim() || tr("entries.new"),
        start: start.toISOString(),
        end: end.toISOString(),
        location: location.trim(),
        all_day: draft.all_day ?? false,
        description: draft.description || undefined,
      });
      onCreated(r.event, r.undo);
    } catch (err) {
      showToast({ title: t("entries.createFailed"), detail: String(err), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };
  const keys = (e: ReactKeyboardEvent) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") {
      e.preventDefault();
      void create();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  };
  return (
    <aside className="calv-detail calv-entry-editor" id="calv-detail" aria-label={t("entries.editor")} onKeyDown={keys}>
      <div className="calv-detail-head">
        <span className="calv-detail-source">
          <CalendarPlus size={13} strokeWidth={1.75} aria-hidden />
          <span className="ellipsis">{t("entries.editor")}</span>
        </span>
        <IconBtn icon={X} label={t("calv.closeEsc")} onClick={onClose} className="icon-btn-sm" />
      </div>
      <div className="calv-detail-top">
        <input autoFocus className="input calv-entry-name" value={title} placeholder={t("entries.titlePlaceholder")} aria-label={t("entries.title")} onChange={(e) => setTitle(e.target.value)} />
        {draft.all_day ? (
          <div className="calv-detail-when">{`${draft.start.toLocaleDateString(dateLocale(), { weekday: "short", day: "numeric", month: "long" })} · ${t("cal.allDay")}`}</div>
        ) : (
          <WhenInputs
            start={draft.start}
            end={draft.end}
            onCommit={(start, end) => {
              when.current = { ...draft, start, end };
              onChange({ ...draft, start, end });
            }}
          />
        )}
      </div>
      <section className="calv-detail-sec calv-entry-fields">
        <input className="input" value={location} placeholder={t("entries.locationPlaceholder")} aria-label={t("entries.location")} onChange={(e) => setLocation(e.target.value)} />
        {calendars.length > 1 && (
          <select className="select" aria-label={t("entries.calendar")} value={target ?? ""} onChange={(e) => setTarget(Number(e.target.value))}>
            {calendars.map((c) => (
              <option key={c.id} value={c.id}>
                {c.source_name && c.source_name !== c.name ? `${c.name} · ${c.source_name}` : c.name}
              </option>
            ))}
          </select>
        )}
        {draft.description && <p className="muted calv-entry-from">{draft.description}</p>}
      </section>
      <div className="calv-detail-actions calv-entry-actions">
        <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void create()}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("entries.create")}</span>
        </button>
        <button type="button" className="btn btn-quiet" onClick={onClose}>
          {t("common.cancel")}
        </button>
      </div>
    </aside>
  );
}

const FIELD_LABELS: Record<"title" | "start" | "end" | "location", TKey> = { title: "entries.title", start: "entries.start", end: "entries.end", location: "entries.location" };

function fieldText(k: keyof typeof FIELD_LABELS, v: string | undefined) {
  if (v == null) return "";
  return k === "start" || k === "end" ? `${new Date(v).toLocaleDateString(dateLocale(), { day: "numeric", month: "numeric" })} ${hhmm(new Date(v))}` : v || "–";
}

function EventDetail({ event: e, calendars, onClose, onSave, onDelete, onReload }: { event: CalendarEvent; calendars: CalendarInfo[]; onClose: () => void; onSave: (p: CalFields) => void; onDelete: () => void; onReload: () => void }) {
  const t = useT();
  const l = dateLocale();
  const [allPeople, setAllPeople] = useState(false);
  const [title, setTitle] = useState(e.title);
  const [location, setLocation] = useState(e.location);
  const [busy, setBusy] = useState("");
  useEffect(() => setAllPeople(false), [e.key]);
  useEffect(() => setTitle(e.title), [e.key, e.title]);
  useEffect(() => setLocation(e.location), [e.key, e.location]);
  const start = new Date(e.start);
  const end = new Date(e.end);
  const multiDay = isoDay(start) !== isoDay(new Date(end.getTime() - 1));
  const minutes = durationMinutes(e);
  const hours = minutes >= 60 ? `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")} h` : `${minutes} min`;
  const when = e.all_day
    ? multiDay
      ? `${start.toLocaleDateString(l, { day: "numeric", month: "long" })} – ${new Date(end.getTime() - 1).toLocaleDateString(l, { day: "numeric", month: "long" })} · ${t("cal.allDay")}`
      : `${start.toLocaleDateString(l, { weekday: "short", day: "numeric", month: "long" })} · ${t("cal.allDay")}`
    : `${start.toLocaleDateString(l, { weekday: "short", day: "numeric", month: "long" })} · ${timeRange(e)} (${hours})`;
  const people = (allPeople ? e.attendees : e.attendees.slice(0, 8)).map(attendeeName);
  const calendar = calendars.find((c) => c.id === e.calendar_id);
  const past = !e.all_day && end.getTime() <= clockNow().getTime();
  const editable = canEdit(e);
  const w = e.write;
  const ro = READ_ONLY[e.editable];
  const conflictFields = w?.state === "conflict" ? (Object.keys(FIELD_LABELS) as (keyof typeof FIELD_LABELS)[]).filter((k) => w.server?.[k] != null || w.mine?.[k] != null) : [];
  const run = async (what: string, fn: () => Promise<unknown>, failed: TKey) => {
    setBusy(what);
    try {
      await fn();
      onReload();
    } catch (err) {
      showToast({ title: t(failed), detail: String(err), tone: "danger" });
    } finally {
      setBusy("");
    }
  };
  const blurOnEnter = (ev: ReactKeyboardEvent<HTMLInputElement>, reset: () => void) => {
    if (ev.nativeEvent.isComposing) return;
    if (ev.key === "Enter") {
      ev.preventDefault();
      ev.currentTarget.blur();
    }
    if (ev.key === "Escape") {
      ev.stopPropagation();
      reset();
    }
  };
  return (
    <aside className="calv-detail" id="calv-detail" aria-label={t("calv.appointment")} style={eventStyle(e, calendars)}>
      <div className="calv-detail-head">
        <span className="calv-detail-source">
          <span className="calv-dot" aria-hidden />
          <span className="ellipsis">{calendar ? (calendar.source_name && calendar.source_name !== calendar.name ? `${calendar.name} · ${calendar.source_name}` : calendar.name) : t("nav.calendar")}</span>
        </span>
        <IconBtn icon={X} label={t("calv.closeEsc")} onClick={onClose} className="icon-btn-sm" />
      </div>
      <div className="calv-detail-top">
        {editable ? (
          <input
            className="input calv-entry-name"
            value={title}
            aria-label={t("entries.title")}
            onChange={(ev) => setTitle(ev.target.value)}
            onBlur={() => {
              const v = title.trim();
              if (v && v !== e.title) onSave({ title: v });
              else setTitle(e.title);
            }}
            onKeyDown={(ev) => blurOnEnter(ev, () => setTitle(e.title))}
          />
        ) : (
          <h2 className="calv-detail-title" tabIndex={-1}>
            {e.title}
          </h2>
        )}
        {editable && !e.all_day ? (
          <WhenInputs start={start} end={end} onCommit={(s, en) => onSave({ start: s.toISOString(), end: en.toISOString() })} />
        ) : (
          <div className="calv-detail-when">
            <span>{when}</span>
            {e.recurring && (
              <span className="calv-detail-series">
                <Repeat size={12} strokeWidth={1.75} aria-hidden />
                {t("calv.partOfSeries")}
              </span>
            )}
          </div>
        )}
        {e.link && (
          <button type="button" className={`btn ${past ? "" : "btn-primary"} calv-join`} onClick={() => void openExternal(e.link!).catch((err) => showToast({ title: t("calv.joinFailed"), detail: String(err), tone: "danger" }))}>
            <Video size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("meeting.join", { kind: e.link_kind ?? t("meeting.online") })}</span>
          </button>
        )}
      </div>

      {editable && (
        <section className="calv-detail-sec calv-entry-fields">
          <input
            className="input calv-entry-place"
            value={location}
            placeholder={t("entries.locationPlaceholder")}
            aria-label={t("entries.location")}
            onChange={(ev) => setLocation(ev.target.value)}
            onBlur={() => location.trim() !== e.location && onSave({ location: location.trim() })}
            onKeyDown={(ev) => blurOnEnter(ev, () => setLocation(e.location))}
          />
        </section>
      )}

      {(w || ro || e.reanswer || e.editable === "confirm" || editable) && (
        <section className="calv-detail-sec calv-entry-state">
          {w?.state === "pending" && (
            <div className="calv-entry-note pending" role="status" data-write="pending">
              <CalendarClock size={14} strokeWidth={1.75} aria-hidden />
              <span>
                {t("entries.pending")}
                {w.error && <span className="muted calv-entry-error">{w.error}</span>}
              </span>
              {w.error && (
                <button type="button" className="btn btn-sm btn-quiet" disabled={busy === "discard"} onClick={() => void run("discard", () => api.calendarDiscard(e.key), "entries.discardFailed")}>
                  <Undo2 size={13} strokeWidth={1.75} aria-hidden />
                  <span>{t("entries.discard")}</span>
                </button>
              )}
            </div>
          )}
          {w?.state === "refused" && (
            <div className="calv-entry-note conflict" role="status" data-write="refused">
              <AlertTriangle size={14} strokeWidth={1.75} aria-hidden />
              <span>
                <b>{t("entries.refused")}</b>
                {w.error && <span className="muted calv-entry-error">{w.error}</span>}
              </span>
              <button type="button" className="btn btn-sm btn-quiet" disabled={busy === "discard"} onClick={() => void run("discard", () => api.calendarDiscard(e.key), "entries.discardFailed")}>
                <Undo2 size={13} strokeWidth={1.75} aria-hidden />
                <span>{t("entries.discard")}</span>
              </button>
            </div>
          )}
          {w?.state === "conflict" && (
            <div className="calv-entry-conflict" role="status" data-write="conflict">
              <div className="calv-entry-note conflict">
                <AlertTriangle size={14} strokeWidth={1.75} aria-hidden />
                <span>
                  <b>{t("entries.conflict")}</b>
                  <span className="muted calv-entry-error">{t("entries.conflictText")}</span>
                </span>
              </div>
              {conflictFields.length > 0 && (
                <dl className="calv-entry-diff">
                  {conflictFields.map((k) => (
                    <div key={k}>
                      <dt>{t(FIELD_LABELS[k])}</dt>
                      <dd>
                        <span className="muted">{t("entries.onServer")}</span> {fieldText(k, w.server?.[k] as string | undefined)}
                      </dd>
                      <dd>
                        <span className="muted">{t("entries.mine")}</span> {fieldText(k, w.mine?.[k] as string | undefined)}
                      </dd>
                    </div>
                  ))}
                </dl>
              )}
              <div className="calv-entry-choice">
                <button type="button" className="btn btn-sm" disabled={!!busy} onClick={() => void run("server", () => api.calendarResolve(e.key, false), "entries.resolveFailed")}>
                  {t("entries.takeServer")}
                </button>
                <button type="button" className="btn btn-sm" disabled={!!busy} onClick={() => void run("mine", () => api.calendarResolve(e.key, true), "entries.resolveFailed")}>
                  {t("entries.keepMine")}
                </button>
              </div>
            </div>
          )}
          {e.editable === "confirm" && (
            <div className="calv-entry-note">
              <Users size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("entries.confirmNote")}</span>
            </div>
          )}
          {e.reanswer && (
            <div className="calv-entry-note" role="status" data-reanswer>
              <CalendarClock size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("entries.reanswer")}</span>
            </div>
          )}
          {ro && (
            <div className="calv-entry-note" data-readonly={e.editable}>
              <Lock size={14} strokeWidth={1.75} aria-hidden />
              <span>{t(ro)}</span>
            </div>
          )}
          {e.editable === "yes" && (
            <div className="calv-entry-buttons">
              <button type="button" className="btn btn-sm btn-quiet calv-entry-delete" onClick={onDelete}>
                <Trash2 size={13} strokeWidth={1.75} aria-hidden />
                <span>{t("entries.delete")}</span>
              </button>
            </div>
          )}
        </section>
      )}

      {((e.location && !editable) || e.organizer || e.attendees.length > 0) && (
        <section className="calv-detail-sec calv-detail-rows">
          {e.location && !editable && (
            <div className="calv-detail-row">
              <MapPin size={14} strokeWidth={1.75} aria-hidden />
              <span>{e.location}</span>
            </div>
          )}
          {e.organizer && (
            <div className="calv-detail-row">
              <User size={14} strokeWidth={1.75} aria-hidden />
              <span>
                {attendeeName(e.organizer)} <span className="muted">({t("calv.organizer")})</span>
              </span>
            </div>
          )}
          {e.attendees.length > 0 && (
            <div className="calv-detail-row">
              <Users size={14} strokeWidth={1.75} aria-hidden />
              <span className="calv-people">
                {people.join(" · ")}
                {e.attendees.length > people.length && (
                  <button type="button" className="link-btn" onClick={() => setAllPeople(true)}>
                    {" "}
                    {t("calv.more", { n: e.attendees.length - people.length })}
                  </button>
                )}
              </span>
            </div>
          )}
        </section>
      )}

      <section className="calv-detail-sec calv-detail-rows calv-detail-facts">
        <div className="calv-detail-row">
          {e.private ? <Lock size={14} strokeWidth={1.75} aria-hidden /> : <Eye size={14} strokeWidth={1.75} aria-hidden />}
          <span>
            {t(BUSY_LABELS[e.busy])}
            {e.private ? ` · ${t("calv.private")}` : ""}
          </span>
        </div>
        {e.categories.length > 0 && (
          <div className="calv-cats">
            {e.categories.map((c) => (
              <span key={c} className="calv-cat">
                {c}
              </span>
            ))}
          </div>
        )}
      </section>

      {e.body && (
        <details className="calv-detail-sec calv-body-text">
          <summary>{t("calv.description")}</summary>
          <pre>{e.body}</pre>
        </details>
      )}
    </aside>
  );
}
