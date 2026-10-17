// The message list's logic without React: which rows are on screen (the list is virtual), which
// are selected after a click with Shift or Ctrl, and what a key does.

/** The rows to render for a scroll position, with some above and below. */
export function visibleRange(scrollTop: number, viewport: number, rowHeight: number, total: number, overscan = 8): { start: number; end: number } {
  if (total <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const first = Math.floor(scrollTop / rowHeight);
  const count = Math.ceil(viewport / rowHeight) + 1;
  return { start: Math.max(0, first - overscan), end: Math.min(total, first + count + overscan) };
}

/** The tallest list drawn as it is; browsers stop at about 33 million pixels (Chromium) or less. */
export const MAX_LIST_PX = 8_000_000;

/**
 * The rows to render for a scroll position, for lists of any length. Up to [`MAX_LIST_PX`] the
 * list is as tall as its rows; a longer one (hundreds of thousands of messages) is drawn that
 * tall and its scroll position is stretched over all rows, so the end stays reachable. `height`:
 * the list's height, `shift`: how far the rows are drawn above their own position, `ratio`: rows'
 * pixels per scrolled pixel.
 */
export function scrollWindow(scrollTop: number, viewport: number, rowHeight: number, total: number, overscan = 8) {
  const full = total * rowHeight;
  const height = Math.min(full, MAX_LIST_PX);
  const ratio = full > height && height > viewport ? (full - viewport) / (height - viewport) : 1;
  const virtual = scrollTop * ratio;
  return { ...visibleRange(virtual, viewport, rowHeight, total, overscan), shift: virtual - scrollTop, height, ratio };
}

/** The pages (of `size` rows) needed to select the rows between the anchor and row `index` that
 *  were never loaded (a range across a page jumped past with the scroll bar). */
export function rangeGaps(sel: Selection, order: (number | undefined)[], index: number, size: number): number[] {
  if (sel.anchor === null) return [];
  const a = order.indexOf(sel.anchor);
  if (a < 0) return [];
  const [lo, hi] = a < index ? [a, index] : [index, a];
  const pages = new Set<number>();
  for (let i = lo; i <= hi; i++) if (order[i] === undefined) pages.add(Math.floor(i / size));
  return [...pages];
}

/** The pages (of `size` rows) a range of rows needs. */
export function pagesFor(start: number, end: number, size: number): number[] {
  if (end <= start) return [];
  const out: number[] = [];
  for (let p = Math.floor(start / size); p <= Math.floor((end - 1) / size); p++) out.push(p);
  return out;
}

export type Selection = { ids: number[]; anchor: number | null };

/** The selection after a click on row `index` (`order`: the ids in list order, as loaded). */
export function clickSelect(sel: Selection, order: (number | undefined)[], index: number, mods: { shift: boolean; toggle: boolean }): Selection {
  const id = order[index];
  if (id === undefined) return sel;
  if (mods.shift && sel.anchor !== null) {
    const a = order.indexOf(sel.anchor);
    if (a >= 0) {
      const [lo, hi] = a < index ? [a, index] : [index, a];
      const range = order.slice(lo, hi + 1).filter((x): x is number => x !== undefined);
      return { ids: [...new Set([...sel.ids, ...range])], anchor: sel.anchor };
    }
  }
  if (mods.toggle) {
    const has = sel.ids.includes(id);
    return { ids: has ? sel.ids.filter((x) => x !== id) : [...sel.ids, id], anchor: id };
  }
  return { ids: [id], anchor: id };
}

export type KeyAction =
  | "next"
  | "prev"
  | "open"
  | "archive"
  | "delete"
  | "reply"
  | "replyAll"
  | "forward"
  | "compose"
  | "search"
  | "undo"
  | "select"
  | "flag"
  | "markRead"
  | "markUnread"
  | "spam"
  | "escape"
  | "settings"
  | "palette"
  | "snooze"
  | "pin"
  | "more";

/** The shortcut a key press means, `null` for anything else. Typing in a field never counts
 *  (except Escape). */
export function keyAction(e: { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }, typing: boolean): KeyAction | null {
  if (e.key === "Escape") return "escape";
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key === ",") return "settings";
  if (mod && e.key.toLowerCase() === "k") return "palette";
  if (mod && e.key.toLowerCase() === "n") return "compose";
  if (typing || mod || e.altKey) return null;
  switch (e.key) {
    case "j":
    case "ArrowDown":
      return "next";
    case "k":
    case "ArrowUp":
      return "prev";
    case "Enter":
    case "o":
      return "open";
    case "e":
      return "archive";
    case "#":
    case "Delete":
      return "delete";
    case "r":
      return "reply";
    case "a":
      return "replyAll";
    case "f":
      return "forward";
    case "c":
      return "compose";
    case "/":
      return "search";
    case "u":
      return "undo";
    case "x":
      return "select";
    case "s":
      return "flag";
    case "!":
      return "spam";
    case "h":
      return "snooze";
    case "p":
      return "pin";
    case ".":
      return "more";
    case "I":
      return "markRead";
    case "U":
      return "markUnread";
    default:
      return null;
  }
}

/** Whether a key press goes into a field (an input, a text area, the editor). */
export function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  const tag = el.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || el.isContentEditable;
}

/** Whether a key press lands on a control of its own (a button, a link, a tab, a switch) outside
 *  the message list, where Enter is that control's. */
export function isControl(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || typeof el.closest !== "function") return false;
  if (el.closest(".messages")) return false;
  return !!el.closest("button, a[href], summary, [role=button], [role=link], [role=tab], [role=switch], [role=menuitem], [role=menuitemradio], [role=option], [role=checkbox]");
}
