// Dates, names and sizes as the list and the reading pane show them.

import { t, type Lang } from "./i18n";
import type { Address } from "./types";

const locale = (lang: Lang) => (lang === "de" ? "de-DE" : "en-GB");

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Today: the time; yesterday: „Gestern“; this year: day and month; older: the full date. */
export function listDate(unixSecs: number, now: Date, lang: Lang): string {
  const d = new Date(unixSecs * 1000);
  const day = startOfDay(d);
  const today = startOfDay(now);
  if (day === today) return d.toLocaleTimeString(locale(lang), { hour: "2-digit", minute: "2-digit" });
  // Calendar days, not 24 hours: the day before a clock change is 23 or 25 hours long.
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime();
  if (day === yesterday) return t("date.yesterday", undefined, lang);
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString(locale(lang), { day: "numeric", month: "short" });
  return d.toLocaleDateString(locale(lang), { day: "2-digit", month: "2-digit", year: "numeric" });
}

/** The full date and time in the reading pane. */
export function fullDate(unixSecs: number, lang: Lang): string {
  return new Date(unixSecs * 1000).toLocaleString(locale(lang), { dateStyle: "full", timeStyle: "short" });
}

/** Date and time, short (collapsed messages of a conversation). */
export function shortDate(unixSecs: number, now: Date, lang: Lang): string {
  const d = new Date(unixSecs * 1000);
  const time = d.toLocaleTimeString(locale(lang), { hour: "2-digit", minute: "2-digit" });
  return startOfDay(d) === startOfDay(now) ? time : `${listDate(unixSecs, now, lang)}, ${time}`;
}

export function displayName(a: Address | null, lang: Lang): string {
  if (!a) return t("list.unknownSender", undefined, lang);
  return a.name?.trim() || a.email;
}

export function addressLine(list: Address[]): string {
  return list.map((a) => (a.name ? `${/[,;"]/.test(a.name) ? `"${a.name.replace(/"/g, "")}"` : a.name} <${a.email}>` : a.email)).join(", ");
}

/** Up to two letters for the sender's circle („Jonas Berger“ → „JB“). */
export function initials(a: Address | null): string {
  const name = (a?.name?.trim() || a?.email.split("@")[0] || "?").replace(/[^\p{L}\p{N} .-]/gu, "");
  const parts = name.split(/[\s.-]+/).filter(Boolean);
  const letters = parts.length >= 2 ? parts[0][0] + parts[parts.length - 1][0] : (parts[0] ?? "?").slice(0, 2);
  return letters.toUpperCase();
}

/** 1,2 MB / 1.2 MB, with the unit after a space. */
export function fileSize(bytes: number, lang: Lang): string {
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  const n = i === 0 ? String(Math.round(v)) : v.toLocaleString(locale(lang), { maximumFractionDigits: v < 10 ? 1 : 0 });
  return `${n} ${units[i]}`;
}
