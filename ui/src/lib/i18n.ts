// Display language: German and English (the catalogs in `ui/src/locales/`, rules in docs/i18n.md).
// `t(key, vars)` translates in the current language: `{name}` is replaced by `vars.name`, and a
// plural entry (`{ one, other }`) picks its form by `vars.n` with Intl.PluralRules. Components
// call `useT()`, which re-renders them when the language changes.

import { useSyncExternalStore } from "react";
import { en, type Msg, type TKey } from "../locales/en";
import { de } from "../locales/de";

export type { TKey };
export type Lang = "de" | "en";
export type TVars = Record<string, string | number>;

export const DICTS: Record<Lang, Record<TKey, Msg>> = { en, de };

/** "de" for a German locale tag („de“, „de-AT“, „de_CH.UTF-8“), else "en". */
export const langFromLocale = (tag: string | undefined | null): Lang => (/^de(?:$|[-_.@])/i.test(tag?.trim() ?? "") ? "de" : "en");

let lang: Lang = typeof navigator === "undefined" ? "en" : langFromLocale(navigator.language);
const listeners = new Set<() => void>();

export const currentLang = () => lang;

export function setLang(l: Lang) {
  if (l === lang) return;
  lang = l;
  if (typeof document !== "undefined") document.documentElement.lang = l;
  for (const f of listeners) f();
}

const plurals: Partial<Record<Lang, Intl.PluralRules>> = {};
const pluralOf = (l: Lang) => (plurals[l] ??= new Intl.PluralRules(l));

export function t(key: TKey, vars?: TVars, l: Lang = lang): string {
  const msg = DICTS[l][key] ?? DICTS.en[key] ?? key;
  let s: string;
  if (typeof msg === "string") s = msg;
  else {
    const n = Number(vars?.n ?? vars?.count ?? 0);
    s = pluralOf(l).select(n) === "one" ? msg.one : msg.other;
  }
  return vars ? s.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m)) : s;
}

const subscribe = (f: () => void) => {
  listeners.add(f);
  return () => listeners.delete(f);
};

/** `t` for components: re-renders when the language changes. */
export function useT(): typeof t {
  useSyncExternalStore(subscribe, currentLang, currentLang);
  return t;
}
