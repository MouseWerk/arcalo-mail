// Dates and names in the message list.

import { t, type Lang } from "./i18n";
import type { Address } from "./types";

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Today: the time; yesterday: „Gestern“; this year: day and month; older: the full date. */
export function listDate(unixSecs: number, now: Date, lang: Lang): string {
  const d = new Date(unixSecs * 1000);
  const day = startOfDay(d);
  const today = startOfDay(now);
  const locale = lang === "de" ? "de-DE" : "en-GB";
  if (day === today) return d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
  // Calendar days, not 24 hours: the day before a clock change is 23 or 25 hours long.
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime();
  if (day === yesterday) return t("date.yesterday", undefined, lang);
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString(locale, { day: "numeric", month: "short" });
  return d.toLocaleDateString(locale, { day: "2-digit", month: "2-digit", year: "numeric" });
}

/** The full date and time in the reading pane. */
export function fullDate(unixSecs: number, lang: Lang): string {
  return new Date(unixSecs * 1000).toLocaleString(lang === "de" ? "de-DE" : "en-GB", { dateStyle: "full", timeStyle: "short" });
}

export function displayName(a: Address | null, lang: Lang): string {
  if (!a) return t("list.unknownSender", undefined, lang);
  return a.name?.trim() || a.email;
}

export function addressLine(list: Address[]): string {
  return list.map((a) => (a.name ? `${a.name} <${a.email}>` : a.email)).join(", ");
}
