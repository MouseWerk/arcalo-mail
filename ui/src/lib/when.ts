// Times of the time tools: the presets of „Zurückstellen“, „Später senden“ and „Erinnern, wenn
// keine Antwort“, and how a time in the near future is named („Morgen, 08:00“).

import { t, type Lang } from "./i18n";

export type SnoozePreset = "laterToday" | "tomorrow" | "weekend" | "nextWeek";
export type SendPreset = "tomorrowMorning" | "tomorrowAfternoon" | "nextWeek";

/** Morning hour of „Morgen“ and „Nächste Woche“. */
const MORNING = 8;

const at = (d: Date, days: number, hour: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + days, hour, 0, 0, 0);

/** The next Monday after today (a Sunday's is the Monday after tomorrow's). */
function nextMonday(now: Date): Date {
  const days = ((8 - now.getDay()) % 7) + (now.getDay() === 0 ? 7 : 0);
  return at(now, days === 0 ? 7 : days, MORNING);
}

/** The presets of „Zurückstellen“ with their times (Unix seconds); „Später heute“ only while
 *  three hours from now are still today and before 22:00, „Am Wochenende“ only Monday to Thursday. */
export function snoozePresets(now: Date): { id: SnoozePreset; at: number }[] {
  const out: { id: SnoozePreset; at: number }[] = [];
  const later = new Date(now.getTime() + 3 * 3600_000);
  later.setMinutes(later.getMinutes() > 0 || later.getSeconds() > 0 ? 60 : 0, 0, 0);
  if (later.getDate() === now.getDate() && later.getHours() <= 22 && later.getHours() > now.getHours()) out.push({ id: "laterToday", at: later.getTime() / 1000 });
  out.push({ id: "tomorrow", at: at(now, 1, MORNING).getTime() / 1000 });
  const day = now.getDay();
  if (day >= 1 && day <= 4) out.push({ id: "weekend", at: at(now, 6 - day, 9).getTime() / 1000 });
  out.push({ id: "nextWeek", at: nextMonday(now).getTime() / 1000 });
  return out;
}

/** The presets of „Später senden“. */
export function sendPresets(now: Date): { id: SendPreset; at: number }[] {
  return [
    { id: "tomorrowMorning", at: at(now, 1, MORNING).getTime() / 1000 },
    { id: "tomorrowAfternoon", at: at(now, 1, 13).getTime() / 1000 },
    { id: "nextWeek", at: nextMonday(now).getTime() / 1000 },
  ];
}

/** „Erinnern, wenn keine Antwort“: after how long. */
export const FOLLOWUP_DAYS = [1, 3, 7] as const;

const locale = (lang: Lang) => (lang === "de" ? "de-DE" : "en-GB");

/** A time in the near future: „Heute, 17:00“, „Morgen, 08:00“, „Mo., 08:00“ within a week, else
 *  „12. März, 08:00“. */
export function whenLabel(unixSecs: number, now: Date, lang: Lang): string {
  const d = new Date(unixSecs * 1000);
  const time = d.toLocaleTimeString(locale(lang), { hour: "2-digit", minute: "2-digit" });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(d) - day(now)) / 86400_000);
  if (diff === 0) return t("when.today", { time }, lang);
  if (diff === 1) return t("when.tomorrow", { time }, lang);
  if (diff === -1) return t("when.yesterday", { time }, lang);
  if (diff > 1 && diff < 7) return `${d.toLocaleDateString(locale(lang), { weekday: "short" })}, ${time}`;
  return `${d.toLocaleDateString(locale(lang), { day: "numeric", month: "short", ...(d.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}) })}, ${time}`;
}

/** The value of an `<input type="datetime-local">` for a time. */
export function toLocalInput(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The time of an `<input type="datetime-local">` value (seconds allowed), `null` if empty or
 *  not a time. */
export function fromLocalInput(value: string): number | null {
  const m = /^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d)(?::(\d\d))?/.exec(value.trim());
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]), Number(m[6] ?? 0));
  return Number.isNaN(d.getTime()) ? null : Math.floor(d.getTime() / 1000);
}
