// Phones: the keyboard, the system's text size and the app's visibility.
//
// - The keyboard: iOS lays it over the page instead of making the page smaller. The visual
//   viewport says how much is left; the app's height follows it (`--app-h`, app.css), so the
//   compose sheet's fields and its send button stay above the keyboard, and a focused field is
//   scrolled into what is left. Android makes the web view smaller itself (SystemBars.kt).
// - The system bars: Android draws the app edge to edge and its WebView reports no safe-area
//   insets, so the shell hands them over as `--sat`, `--sar`, `--sab`, `--sal` and `--ime`
//   (CSS pixels, on <html>); `--safe-*` (tokens.css) takes them or iOS's env() values.
// - The text size: iOS's Dynamic Type reaches the page only through `font: -apple-system-body`;
//   its size sets `--type-scale` (touch.ts), which the touch type scale multiplies (app.css).
//   Android's font size comes from the shell as `--type-scale` too (SystemBars.kt).
// - Visibility: the app tells the shell when it is hidden or shown again; a phone keeps its
//   connections only while the app is in front.

import { typeScale } from "./touch";

export function watchViewport(root: HTMLElement = document.documentElement): () => void {
  const vv = window.visualViewport;
  if (!vv) return () => {};
  const update = () => {
    root.style.setProperty("--app-h", `${Math.round(vv.height)}px`);
    root.toggleAttribute("data-keyboard", window.innerHeight - vv.height > 120);
    // iOS scrolls the whole page to show a focused field; the app scrolls its own panes.
    if (window.scrollY !== 0) window.scrollTo(0, 0);
  };
  const onFocus = (e: FocusEvent) => {
    const el = e.target as HTMLElement | null;
    if (!el || !el.matches?.("input, textarea, [contenteditable], select")) return;
    // After the keyboard came up.
    setTimeout(() => el.scrollIntoView?.({ block: "nearest" }), 300);
  };
  vv.addEventListener("resize", update);
  vv.addEventListener("scroll", update);
  document.addEventListener("focusin", onFocus);
  update();
  return () => {
    vv.removeEventListener("resize", update);
    vv.removeEventListener("scroll", update);
    document.removeEventListener("focusin", onFocus);
    root.style.removeProperty("--app-h");
  };
}

/** iOS: the Dynamic Type body size as the app's type scale (1 elsewhere). */
export function applyTypeScale(root: HTMLElement = document.documentElement): void {
  if (typeof CSS === "undefined" || !CSS.supports?.("font", "-apple-system-body")) return;
  const probe = document.createElement("span");
  probe.style.font = "-apple-system-body";
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  document.body.appendChild(probe);
  const px = parseFloat(getComputedStyle(probe).fontSize);
  probe.remove();
  root.style.setProperty("--type-scale", String(typeScale(px)));
}

/** Calls `f(visible)` now and whenever the app is hidden or shown again. */
export function onVisibility(f: (visible: boolean) => void): () => void {
  const update = () => f(document.visibilityState === "visible");
  document.addEventListener("visibilitychange", update);
  update();
  return () => document.removeEventListener("visibilitychange", update);
}

/** The Android shell's side (SystemBars.kt): the bars' insets and the system's font size, and
 *  the page's theme for the bars' icons and the window behind the page. */
type SystemBarsBridge = { insets?: () => string; setTheme?: (dark: boolean, background: string) => void };
const bridge = (): SystemBarsBridge | undefined => (typeof window === "undefined" ? undefined : (window as unknown as { ArcaloSystemBars?: SystemBarsBridge }).ArcaloSystemBars);

/** The only properties the shell may set. */
export const SYSTEM_BAR_PROPS = ["--sat", "--sar", "--sab", "--sal", "--ime", "--type-scale"] as const;

/** Sets the values the shell reports (a JSON object of `SYSTEM_BAR_PROPS`); others are ignored. */
export function applyInsets(json: string, root: HTMLElement = document.documentElement): boolean {
  let v: Record<string, unknown>;
  try {
    v = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return false;
  }
  if (!v || typeof v !== "object") return false;
  for (const k of SYSTEM_BAR_PROPS) {
    const x = v[k];
    if (typeof x !== "string") continue;
    if (k === "--type-scale" ? /^\d+(\.\d+)?$/.test(x) : /^\d+(\.\d+)?px$/.test(x)) root.style.setProperty(k, x);
  }
  return true;
}

/** At start: what the shell knows already (later changes it sets itself). */
export function readSystemBars(root: HTMLElement = document.documentElement): void {
  const b = bridge();
  if (b?.insets) applyInsets(b.insets(), root);
}

/** Tells the shell the theme in use: dark or light icons on the bars, and its background. */
export function tellSystemBars(dark: boolean, background: string | undefined): void {
  const b = bridge();
  if (b?.setTheme && background) b.setTheme(dark, background);
}
