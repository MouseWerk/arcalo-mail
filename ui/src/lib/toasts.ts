// Short messages at the bottom of the window („Archiviert · Rückgängig“, errors), and the last
// action that `u` takes back.

import { useSyncExternalStore } from "react";

export type Toast = {
  id: number;
  title: string;
  detail?: string;
  tone?: "info" | "success" | "danger";
  action?: { label: string; run: () => void };
  /** Milliseconds; 0 stays until closed. */
  duration?: number;
};

let toasts: Toast[] = [];
let next = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((f) => f());

export function showToast(t: Omit<Toast, "id">): number {
  const id = next++;
  toasts = [...toasts.slice(-3), { ...t, id }];
  emit();
  const d = t.duration ?? 5000;
  if (d > 0) setTimeout(() => closeToast(id), d);
  return id;
}

export function closeToast(id: number) {
  if (!toasts.some((t) => t.id === id)) return;
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

const subscribe = (f: () => void) => {
  listeners.add(f);
  return () => listeners.delete(f);
};

export function useToasts(): Toast[] {
  return useSyncExternalStore(subscribe, () => toasts, () => toasts);
}

/** The last action that can be taken back (`u`, „Rückgängig“). */
let lastUndo: { run: () => void; until: number } | null = null;

export function setUndo(run: () => void, ms = 5000) {
  lastUndo = { run, until: Date.now() + ms };
}

export function runUndo(): boolean {
  if (!lastUndo || Date.now() > lastUndo.until) return false;
  const u = lastUndo;
  lastUndo = null;
  u.run();
  return true;
}
