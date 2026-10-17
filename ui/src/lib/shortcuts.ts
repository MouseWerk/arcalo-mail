// The keyboard shortcuts as Settings → Tastenkürzel lists them, with the modifier of the system:
// ⌘ on macOS (and iPadOS with a keyboard), Strg/Ctrl elsewhere. The keys are those of
// `keyAction` (listing.ts) and the compose sheet.

import type { TKey } from "./i18n";

/** One key of a combination: a letter or sign as it is, or a named key. */
export type KeyToken = "Mod" | "Shift" | "Enter" | "Delete" | string;

/** A shortcut: its alternatives (each a combination of keys) and what it does. */
export type Shortcut = { keys: KeyToken[][]; label: TKey };

const k = (...alternatives: string[]): KeyToken[][] => alternatives.map((a) => a.split("+"));

export const SHORTCUTS: Shortcut[] = [
  { keys: k("j", "k"), label: "keys.nextPrev" },
  { keys: k("Enter", "o"), label: "keys.open" },
  { keys: k("e"), label: "keys.archive" },
  { keys: k("#", "Delete"), label: "keys.delete" },
  { keys: k("!"), label: "keys.spam" },
  { keys: k("r"), label: "keys.reply" },
  { keys: k("a"), label: "keys.replyAll" },
  { keys: k("f"), label: "keys.forward" },
  { keys: k("c", "Mod+N"), label: "keys.compose" },
  { keys: k("/"), label: "keys.search" },
  { keys: k("u"), label: "keys.undo" },
  { keys: k("x"), label: "keys.select" },
  { keys: k("s"), label: "keys.flag" },
  { keys: k("h"), label: "keys.snooze" },
  { keys: k("p"), label: "keys.pin" },
  { keys: k("."), label: "keys.more" },
  { keys: k("Shift+I", "Shift+U"), label: "keys.readUnread" },
  { keys: k("Mod+Enter"), label: "keys.send" },
  { keys: k("Mod+S"), label: "keys.saveDraft" },
  { keys: k("Mod+K"), label: "keys.palette" },
  { keys: k("Mod+,"), label: "keys.settings" },
];

/** Whether the system's shortcut modifier is ⌘. */
export const usesCommand = (platform: string | undefined) => platform === "macos" || platform === "ios";

/** How a key is written on this system (`names`: the catalog's words for Ctrl, Shift, …). */
export function keyLabel(token: KeyToken, mac: boolean, names: { ctrl: string; shift: string; enter: string; del: string }): string {
  switch (token) {
    case "Mod":
      return mac ? "⌘" : names.ctrl;
    case "Shift":
      return mac ? "⇧" : names.shift;
    case "Enter":
      return mac ? "↩" : names.enter;
    case "Delete":
      return names.del;
    default:
      return token;
  }
}
