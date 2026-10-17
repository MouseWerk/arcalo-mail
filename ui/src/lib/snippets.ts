// Snippets in the compose sheet: typing `;;` and the start of a trigger word offers the
// snippets that fit; Enter or Tab puts the chosen one in place of what was typed.

import type { Template } from "./types";

/** The trigger in front of the caret: `;;dan` → `dan` (`""` right after `;;`), else `null`. */
export function snippetQuery(before: string): string | null {
  const m = /(?:^|[\s(]);;([\p{L}\p{N}_-]{0,30})$/u.exec(before);
  return m ? m[1].toLowerCase() : null;
}

/** The snippets for a query, the exact trigger first, then by trigger and name. */
export function matchSnippets(list: Template[], query: string, accountId: number | null): Template[] {
  const q = query.toLowerCase();
  return list
    .filter((t) => t.kind === "snippet" && (t.account_id == null || t.account_id === accountId))
    .filter((t) => !q || (t.trigger ?? "").startsWith(q) || t.name.toLowerCase().includes(q))
    .sort((a, b) => Number((b.trigger ?? "") === q) - Number((a.trigger ?? "") === q) || (a.trigger ?? "").localeCompare(b.trigger ?? ""))
    .slice(0, 8);
}

/** Today in the user's words, for `{date}` and `{time}`. */
export function today(lang: "de" | "en", at: Date = new Date()): { date: string; time: string } {
  const locale = lang === "de" ? "de-DE" : "en-GB";
  return {
    date: at.toLocaleDateString(locale, { day: "numeric", month: "long", year: "numeric" }),
    time: at.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" }),
  };
}
