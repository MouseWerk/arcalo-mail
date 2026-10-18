// Phones and tablets: which layout a window gets, the swipe over a row of the list, pull to
// refresh, the system's text size on iOS, and what Back does. Pure functions, so the rules are
// tested without a touch screen (lib.test.ts); the components only measure and call them.

import type { SwipeAction } from "./types";

/** `phone`: one pane at a time; `tablet`: the list and the message side by side, the folders as
 *  a drawer; `desktop`: three panes. */
export type Layout = "phone" | "tablet" | "desktop";

/** The narrowest window that shows two panes (phones in landscape, small tablets). */
export const PHONE_MAX = 720;
/** Lower than this (a phone in landscape), writing and the settings take the whole screen. */
export const SHORT_MAX = 560;
/** Touch screens up to this width keep the folders in a drawer. */
export const TABLET_MAX = 1100;

export function layoutFor(width: number, touch: boolean): Layout {
  if (width <= PHONE_MAX) return "phone";
  if (touch && width <= TABLET_MAX) return "tablet";
  return "desktop";
}

/** Whether this device is used with a finger (the app on a phone, or a touch screen as the
 *  only pointer). */
export function isTouch(appSaysTouch: boolean): boolean {
  if (appSaysTouch) return true;
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia("(pointer: coarse)").matches && !window.matchMedia("(pointer: fine)").matches;
}

// ---------------------------------------------------------------- swipe

/** A swipe counts once the row moved this far (pixels) or this share of its width. */
export const SWIPE_MIN = 96;
export const SWIPE_SHARE = 0.32;
/** Movement before the gesture decides between swiping and scrolling. */
export const SWIPE_SLOP = 10;

/** Whether a finger that moved (dx, dy) swipes the row (sideways) or scrolls the list. */
export function swipeIntent(dx: number, dy: number): "swipe" | "scroll" | null {
  if (Math.abs(dx) < SWIPE_SLOP && Math.abs(dy) < SWIPE_SLOP) return null;
  return Math.abs(dx) > Math.abs(dy) * 1.2 ? "swipe" : "scroll";
}

/** The action a swipe would take: to the left (dx < 0) `left`, to the right `right`; `none`
 *  sides do not move. */
export function swipeSide(dx: number, left: SwipeAction, right: SwipeAction): SwipeAction | null {
  const action = dx < 0 ? left : right;
  return action === "none" || dx === 0 ? null : action;
}

/** Where the row is drawn for a finger at `dx`: free up to the threshold, then with resistance;
 *  a side without an action barely moves. */
export function swipeOffset(dx: number, width: number, left: SwipeAction, right: SwipeAction): number {
  if (!swipeSide(dx, left, right)) return Math.sign(dx) * Math.min(Math.abs(dx), 12) * 0.5;
  const limit = swipeThreshold(width);
  const d = Math.abs(dx);
  const drawn = d <= limit ? d : limit + (d - limit) * 0.35;
  return Math.sign(dx) * Math.min(drawn, width);
}

export function swipeThreshold(width: number): number {
  return Math.min(SWIPE_MIN, Math.max(48, width * SWIPE_SHARE));
}

/** The action when the finger lifts at `dx`, or null when the row springs back. */
export function swipeResult(dx: number, width: number, left: SwipeAction, right: SwipeAction): SwipeAction | null {
  const action = swipeSide(dx, left, right);
  return action && Math.abs(dx) >= swipeThreshold(width) ? action : null;
}

/** Actions that take the row out of the list (it slides away); the others spring back. */
export function swipeRemoves(action: SwipeAction): boolean {
  return action === "archive" || action === "delete";
}

// ---------------------------------------------------------------- pull to refresh

/** How far the list must be pulled (drawn distance) to check for mail. */
export const PULL_TRIGGER = 64;
export const PULL_MAX = 96;

/** The drawn distance for a finger that moved `dy` down at the top of the list. */
export function pullDistance(dy: number): number {
  if (dy <= 0) return 0;
  return Math.min(PULL_MAX, dy * 0.5);
}

export function pullTriggers(dy: number): boolean {
  return pullDistance(dy) >= PULL_TRIGGER;
}

// ---------------------------------------------------------------- text size

/** iOS: the body size of Dynamic Type (`font: -apple-system-body`, 17 px at the default) as a
 *  factor for the app's type scale, within what the layout takes. */
export function typeScale(bodyPx: number | null): number {
  if (!bodyPx || !Number.isFinite(bodyPx)) return 1;
  return Math.round(Math.min(1.5, Math.max(0.88, bodyPx / 17)) * 100) / 100;
}

// ---------------------------------------------------------------- back

export type Screen = { view: "folders" | "list" | "message"; layout: Layout; composing: boolean; settings: boolean };

/** What the system's Back (Android's button or gesture) does, or null: leave the app. */
export function backStep(s: Screen): "compose" | "settings" | "list" | "folders" | null {
  if (s.composing) return "compose";
  if (s.settings) return "settings";
  if (s.view === "message" && s.layout === "phone") return "list";
  if (s.view === "folders" && s.layout !== "desktop") return "list";
  return null;
}
