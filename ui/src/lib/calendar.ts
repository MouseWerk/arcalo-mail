// The calendar view's logic (taken from Arcalo's Kalender, lib/agenda.ts, lib/blocks.ts and
// lib/eventlook.ts): the visible ranges, the overlap layout of the time grid, month cells, moving
// and resizing on the 15-minute grid, how much text fits in a block, and the colors of calendars.

import { currentLang } from "./i18n";
import { t } from "./i18n";
import { now as clockNow } from "./format";
import type { CalendarEvent, CalendarInfo } from "./types";

export type CalView = "day" | "workweek" | "week" | "month" | "agenda";

export const CAL_VIEWS: CalView[] = ["day", "workweek", "week", "month", "agenda"];

/** Appointments start and end on this grid (minutes) when dragged. */
export const SNAP = 15;
/** Pixels per hour in the time grid. */
export const HOUR = 48;
/** Days of the list. */
export const AGENDA_DAYS = 14;

export interface Range {
  /** 00:00 local of the first day. */
  from: Date;
  /** 00:00 local after the last day. */
  to: Date;
  days: Date[];
}

export const dateLocale = () => (currentLang() === "de" ? "de-DE" : "en-GB");

/** The system's language tag (the app's `app_info`); the browser's own is often not the system's. */
let weekLocale: string | null = null;
export function setWeekLocale(tag: string | null | undefined) {
  weekLocale = tag?.replace("_", "-").split(".")[0] || null;
}

/** The first day of the week: the system locale's (Intl), Monday when it says nothing. */
export function weekStartsOn(): 0 | 1 {
  try {
    const loc = new Intl.Locale(weekLocale ?? dateLocale()) as Intl.Locale & { weekInfo?: { firstDay: number }; getWeekInfo?: () => { firstDay: number } };
    const first = loc.getWeekInfo?.().firstDay ?? loc.weekInfo?.firstDay;
    return first === 7 ? 0 : 1;
  } catch {
    return 1;
  }
}

export const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
export const addDays = (d: Date, n: number) => {
  const x = new Date(d);
  x.setDate(x.getDate() + n);
  return x;
};
const isoWeekday = (d: Date) => ((d.getDay() + 6) % 7) + 1;

export function isoDay(d: Date) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 00:00 local of the first day of the week containing `d`. */
export function weekStart(d: Date, startsOn: 0 | 1 = weekStartsOn()) {
  const x = midnight(d);
  x.setDate(x.getDate() - ((x.getDay() + 7 - startsOn) % 7));
  return x;
}

export function isoWeek(d: Date) {
  const x = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = x.getUTCDay() || 7;
  x.setUTCDate(x.getUTCDate() + 4 - day);
  const y = new Date(Date.UTC(x.getUTCFullYear(), 0, 1));
  return Math.ceil(((x.getTime() - y.getTime()) / 86400000 + 1) / 7);
}

/** The ISO week of a week shown from `start`: one that starts on Sunday counts as its Monday's. */
export const shownWeek = (start: Date) => isoWeek(start.getDay() === 0 ? addDays(start, 1) : start);

/** Same day `n` months later, clamped to the month's last day. */
export function addMonths(d: Date, n: number): Date {
  const target = new Date(d.getFullYear(), d.getMonth() + n, 1);
  const last = new Date(target.getFullYear(), target.getMonth() + 1, 0).getDate();
  return new Date(target.getFullYear(), target.getMonth(), Math.min(d.getDate(), last));
}

/** Six weeks around a month. */
export function monthGrid(year: number, month: number, startsOn: 0 | 1 = weekStartsOn()): Date[] {
  const first = new Date(year, month, 1);
  const start = new Date(year, month, 1 - ((first.getDay() + 7 - startsOn) % 7));
  return Array.from({ length: 42 }, (_, i) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + i));
}

/** The days a view shows around `anchor`; the work week keeps the configured workdays. */
export function viewRange(view: CalView, anchor: Date, startsOn: 0 | 1 = weekStartsOn(), workdays: number[] = [1, 2, 3, 4, 5], today: Date = clockNow()): Range {
  const day = midnight(anchor);
  let days: Date[];
  switch (view) {
    case "day":
      days = [day];
      break;
    case "week":
    case "workweek": {
      const first = weekStart(day, startsOn);
      days = Array.from({ length: 7 }, (_, i) => addDays(first, i));
      if (view === "workweek") {
        const wd = workdays.length ? workdays : [1, 2, 3, 4, 5];
        // Today stays visible on a day off.
        const t0 = midnight(today).getTime();
        const kept = days.filter((d) => wd.includes(isoWeekday(d)) || d.getTime() === t0);
        days = kept.length ? kept : days;
      }
      break;
    }
    case "month":
      days = monthGrid(day.getFullYear(), day.getMonth(), startsOn);
      break;
    case "agenda":
      days = Array.from({ length: AGENDA_DAYS }, (_, i) => addDays(day, i));
      break;
  }
  return { from: days[0], to: addDays(days[days.length - 1], 1), days };
}

/** The anchor one step back (-1) or ahead (+1). */
export function step(view: CalView, anchor: Date, dir: -1 | 1): Date {
  switch (view) {
    case "day":
      return addDays(midnight(anchor), dir);
    case "week":
    case "workweek":
      return addDays(midnight(anchor), 7 * dir);
    case "month":
      return addMonths(midnight(anchor), dir);
    case "agenda":
      return addDays(midnight(anchor), AGENDA_DAYS * dir);
  }
}

/** Heading of the range: „Freitag, 25. September 2026“, „21.–27. September 2026“, „September 2026“. */
export function rangeTitle(view: CalView, range: Range, anchor: Date): string {
  const l = dateLocale();
  if (view === "day") return anchor.toLocaleDateString(l, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
  if (view === "month") return anchor.toLocaleDateString(l, { month: "long", year: "numeric" });
  const a = range.days[0];
  const b = range.days[range.days.length - 1];
  const long = (d: Date) => d.toLocaleDateString(l, { day: "numeric", month: "long", year: "numeric" });
  if (a.getFullYear() !== b.getFullYear()) return `${long(a)} – ${long(b)}`;
  if (a.getMonth() !== b.getMonth()) return `${a.toLocaleDateString(l, { day: "numeric", month: "long" })} – ${long(b)}`;
  return `${a.toLocaleDateString(l, { day: "numeric" })}–${long(b)}`;
}

/** „KW 39“ (or „KW 39–40“); „Week 39“ in English. */
export function weekLabel(range: Range): string {
  const a = range.days.length > 1 ? shownWeek(range.days[0]) : isoWeek(range.days[0]);
  const b = isoWeek(range.days[range.days.length - 1]);
  return t("cal.weekNo", { n: a === b ? String(a) : `${a}–${b}` });
}

interface Timed {
  start: string;
  end: string;
}

export interface Placed<T> {
  item: T;
  /** Minutes after the day's midnight. */
  top: number;
  height: number;
  /** Column in its group of overlapping items, and the group's column count. */
  col: number;
  cols: number;
}

/** Local time of day in minutes. */
export const minutesOfDay = (d: Date) => d.getHours() * 60 + d.getMinutes();

/**
 * Side-by-side layout of timed items on one day: overlapping items form a group; each takes the
 * first free column; all items of a group share its column count. Items shorter than
 * `minMinutes` are drawn as that long, so short meetings stay readable.
 */
export function layoutDay<T extends Timed>(items: T[], day: Date, minMinutes = 20): Placed<T>[] {
  const dayStart = midnight(day).getTime();
  const dayEnd = addDays(midnight(day), 1).getTime();
  const rows = items
    .map((item) => {
      const s = new Date(item.start).getTime();
      return { item, s, e: Math.max(new Date(item.end).getTime(), s) };
    })
    .filter((r) => r.s < dayEnd && (r.e > dayStart || (r.e === r.s && r.s >= dayStart)))
    .map((r) => {
      const from = Math.max(r.s, dayStart);
      const to = Math.min(r.e, dayEnd);
      // Wall-clock minutes like the hour labels (the days the clocks change have 23 or 25 hours).
      const top = from === dayStart ? 0 : minutesOfDay(new Date(from));
      const bottom = to === dayEnd ? 24 * 60 : minutesOfDay(new Date(to));
      const height = Math.max(bottom > top ? bottom - top : (to - from) / 60000, minMinutes);
      return { item: r.item, top: Math.min(top, 24 * 60 - minMinutes), height };
    })
    .sort((a, b) => a.top - b.top || b.height - a.height);
  const out: Placed<T>[] = [];
  let group: Placed<T>[] = [];
  let colEnds: number[] = [];
  let groupEnd = -1;
  const flush = () => {
    for (const p of group) p.cols = colEnds.length;
    out.push(...group);
    group = [];
    colEnds = [];
  };
  for (const r of rows) {
    if (group.length && r.top >= groupEnd) flush();
    let col = colEnds.findIndex((end) => end <= r.top);
    if (col < 0) {
      col = colEnds.length;
      colEnds.push(0);
    }
    colEnds[col] = r.top + r.height;
    groupEnd = group.length ? Math.max(groupEnd, r.top + r.height) : r.top + r.height;
    group.push({ ...r, col, cols: 1 });
  }
  flush();
  return out;
}

/** Whether an item covers (part of) the local day. */
export function onDay(item: Timed, day: Date): boolean {
  const s = new Date(item.start).getTime();
  const e = new Date(item.end).getTime();
  const a = midnight(day).getTime();
  const b = addDays(midnight(day), 1).getTime();
  return s < b && (e > a || (e === s && s >= a));
}

/** All-day row: all-day appointments and those lasting 24 hours or more. */
export const isAllDayLike = (e: Pick<CalendarEvent, "all_day" | "start" | "end">) => e.all_day || new Date(e.end).getTime() - new Date(e.start).getTime() >= 24 * 3600 * 1000;

/** Appointments of a day: all-day first, then by start. */
export function dayEvents(events: CalendarEvent[], d: Date): CalendarEvent[] {
  return events
    .filter((e) => onDay(e, d))
    .sort((a, b) => Number(isAllDayLike(b)) - Number(isAllDayLike(a)) || new Date(a.start).getTime() - new Date(b.start).getTime() || a.title.localeCompare(b.title));
}

export interface DayCell {
  shown: CalendarEvent[];
  /** Not shown („+3 weitere“). */
  more: number;
}

/** Month cells by day (YYYY-MM-DD): at most `max` lines incl. the „weitere“ line. */
export function monthCells(events: CalendarEvent[], days: Date[], max = 4): Map<string, DayCell> {
  const out = new Map<string, DayCell>();
  for (const d of days) {
    const list = dayEvents(events, d);
    const shown = list.length > max ? list.slice(0, max - 1) : list;
    out.set(isoDay(d), { shown, more: list.length - shown.length });
  }
  return out;
}

export const hhmm = (d: Date) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;

/** „10:00–11:30“, or „ganztägig“. */
export function timeRange(e: Pick<CalendarEvent, "all_day" | "start" | "end">): string {
  if (e.all_day) return t("cal.allDay");
  return `${hhmm(new Date(e.start))}–${hhmm(new Date(e.end))}`;
}

/** Minutes of an appointment. */
export const durationMinutes = (e: Timed) => Math.max(0, Math.round((new Date(e.end).getTime() - new Date(e.start).getTime()) / 60000));

/** Minutes on the grid (nearest). */
export const snapMinutes = (m: number, stepMin = SNAP) => Math.round(m / stepMin) * stepMin;

/** The appointment moved by `minutes` (and `days`, keeping the time of day across a clock change). */
export function moved(b: Timed, minutes: number, days = 0): { start: string; end: string } {
  const start = new Date(b.start);
  const length = new Date(b.end).getTime() - start.getTime();
  start.setDate(start.getDate() + days);
  const s = start.getTime() + snapMinutes(minutes) * 60_000;
  return { start: new Date(s).toISOString(), end: new Date(s + length).toISOString() };
}

/** The appointment with its end moved by `minutes`; at least one step long. */
export function resized(b: Timed, minutes: number): { start: string; end: string } {
  const start = new Date(b.start).getTime();
  const end = Math.max(start + SNAP * 60_000, new Date(b.end).getTime() + snapMinutes(minutes) * 60_000);
  return { start: b.start, end: new Date(end).toISOString() };
}

/**
 * The times after the panel's date and time fields (`YYYY-MM-DD`, `HH:MM`) were edited; `null`
 * when they say nothing new or are no time. Another date moves the whole appointment, a new start
 * keeps its length, a new end keeps the days it spans (the next day when it is not after the start).
 */
export function editedTimes(start: Date, end: Date, f: { date: string; from: string; to: string }): { start: Date; end: Date } | null {
  const [y, m, d] = f.date.split("-").map(Number);
  const hm = (v: string) => (/^\d{1,2}:\d{2}$/.test(v.trim()) ? v.trim().split(":").map(Number) : [NaN, NaN]);
  const [h1, m1] = hm(f.from);
  const [h2, m2] = hm(f.to);
  if (![y, m, d, h1, m1, h2, m2].every(Number.isFinite)) return null;
  const length = end.getTime() - start.getTime();
  const s = new Date(y, m - 1, d, h1, m1);
  let e: Date;
  if (`${String(h2).padStart(2, "0")}:${String(m2).padStart(2, "0")}` === hhmm(end)) e = new Date(s.getTime() + length);
  else {
    const days = Math.round((midnight(end).getTime() - midnight(start).getTime()) / 86_400_000);
    e = new Date(y, m - 1, d + days, h2, m2);
    if (e <= s) e = new Date(y, m - 1, d + days + 1, h2, m2);
  }
  return s.getTime() === start.getTime() && e.getTime() === end.getTime() ? null : { start: s, end: e };
}

export interface BlockFit {
  /** Title and start time on one line (short meetings). */
  oneLine: boolean;
  /** Lines the title may take (line clamp). */
  titleLines: number;
  /** The place on its own line. */
  place: boolean;
}

/** Line heights of the block text (12 px title, 11 px meta) and its vertical padding. */
const BLOCK = { title: 16, meta: 14, pad: 6 } as const;

/** What fits into a block of `heightPx` pixels lasting `minutes`. */
export function blockFit(heightPx: number, minutes: number, hasPlace: boolean): BlockFit {
  const room = heightPx - BLOCK.pad;
  if (minutes <= 30 || room < BLOCK.title + BLOCK.meta) return { oneLine: true, titleLines: 1, place: false };
  const place = hasPlace && room >= BLOCK.title + BLOCK.meta * 2;
  const left = room - BLOCK.meta - (place ? BLOCK.meta : 0);
  return { oneLine: false, titleLines: Math.max(1, Math.floor(left / BLOCK.title)), place };
}

/** The keyboard: arrows move, T today, D/A/W/M/L the views (A = Arbeitswoche, L = Liste). */
export function keyAction(key: string): { move?: -1 | 1; today?: true; view?: CalView } | null {
  switch (key) {
    case "ArrowLeft":
    case "PageUp":
      return { move: -1 };
    case "ArrowRight":
    case "PageDown":
      return { move: 1 };
    case "t":
    case "T":
      return { today: true };
    case "d":
    case "D":
      return { view: "day" };
    case "a":
    case "A":
      return { view: "workweek" };
    case "w":
    case "W":
      return { view: "week" };
    case "m":
    case "M":
      return { view: "month" };
    case "l":
    case "L":
      return { view: "agenda" };
    default:
      return null;
  }
}

/** The name of an attendee entry (`Anna Müller <anna@firma.de>` shows as „Anna Müller“). */
export function attendeeName(entry: string): string {
  const e = entry.trim();
  const m = /^(.*\S)\s+<([^<>]*@[^<>]*)>$/.exec(e);
  return m ? m[1].replace(/^"|"$/g, "") : e;
}

/** Color of a calendar (its own, else the theme's accent). */
export function calendarColor(id: number, list: CalendarInfo[]): string {
  return list.find((c) => c.id === id)?.color || "var(--accent)";
}

/** The calendar palette (the core's `PALETTE`). */
export const CAL_PALETTE = ["#2563eb", "#0d9488", "#9333ea", "#ea580c", "#db2777", "#65a30d", "#0891b2", "#ca8a04"];

/** Whether an appointment can be changed here (a meeting the user organizes after a confirmation). */
export const canEdit = (e: Pick<CalendarEvent, "editable">) => e.editable === "yes" || e.editable === "confirm";

/** People invited besides the organizer. */
export function invited(e: CalendarEvent): number {
  const organizer = attendeeName(e.organizer).toLowerCase();
  return e.attendees.filter((a) => a.trim() && attendeeName(a).toLowerCase() !== organizer).length;
}

/** Asks the app to open the calendar: on an appointment (`key`), a day, or with a new one's draft. */
export type CalendarRequest = { key?: string | null; day?: string; draft?: import("./types").EventDraft };

export function openCalendar(r: CalendarRequest) {
  window.dispatchEvent(new CustomEvent<CalendarRequest>("arcalo-mail:calendar", { detail: r }));
}

/** A day as the app writes it in a field: „17.10.2026“ (German), „17/10/2026“ (English). */
export function dayText(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return currentLang() === "de" ? `${p(d.getDate())}.${p(d.getMonth() + 1)}.${d.getFullYear()}` : `${p(d.getDate())}/${p(d.getMonth() + 1)}/${d.getFullYear()}`;
}

/** `YYYY-MM-DD` of a typed day (day first, as `dayText` writes it, or ISO); `null` when it is none. */
export function parseDayText(s: string): string | null {
  const v = s.trim();
  let m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(v);
  let [y, mo, d] = m ? [Number(m[1]), Number(m[2]), Number(m[3])] : [NaN, NaN, NaN];
  if (!m) {
    m = /^(\d{1,2})[./-](\d{1,2})[./-](\d{2}|\d{4})$/.exec(v);
    if (!m) return null;
    [d, mo, y] = [Number(m[1]), Number(m[2]), Number(m[3])];
    if (y < 100) y += 2000;
  }
  const date = new Date(y, mo - 1, d);
  if (date.getFullYear() !== y || date.getMonth() !== mo - 1 || date.getDate() !== d) return null;
  return isoDay(date);
}

/** `HH:MM` of a typed time („9“, „930“, „9:30“, „09.30“); `null` when it is none. */
export function parseTimeText(s: string): string | null {
  const v = s.trim().replace(".", ":");
  const m = /^(\d{1,2})(?::?(\d{2}))?$/.exec(v);
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2] ?? 0);
  if (h > 23 || min > 59) return null;
  return `${String(h).padStart(2, "0")}:${String(min).padStart(2, "0")}`;
}
