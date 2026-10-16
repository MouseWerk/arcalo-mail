// Light, dark and contrast themes: `data-theme` on <html> picks the token set (styles/tokens.css).
// „Wie das System“ follows the operating system's light or dark setting, and its contrast
// preference.

import type { ThemeChoice } from "./types";

export type Theme = "light" | "dark" | "contrast";

export function resolveTheme(choice: ThemeChoice, prefersDark: boolean, prefersContrast: boolean): Theme {
  if (choice !== "system") return choice;
  if (prefersContrast) return "contrast";
  return prefersDark ? "dark" : "light";
}

const media = (q: string) => (typeof window !== "undefined" && window.matchMedia ? window.matchMedia(q).matches : false);

export function applyTheme(choice: ThemeChoice): Theme {
  const theme = resolveTheme(choice, media("(prefers-color-scheme: dark)"), media("(prefers-contrast: more)"));
  document.documentElement.dataset.theme = theme;
  return theme;
}

/** Calls `f` when the system's appearance changes. */
export function onSystemThemeChange(f: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const qs = [window.matchMedia("(prefers-color-scheme: dark)"), window.matchMedia("(prefers-contrast: more)")];
  for (const q of qs) q.addEventListener("change", f);
  return () => qs.forEach((q) => q.removeEventListener("change", f));
}
