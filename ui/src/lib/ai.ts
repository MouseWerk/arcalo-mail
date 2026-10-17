// The AI assistant in the UI: a request runs in the backend (src-tauri/src/assistant.rs), its
// answer streams in through `ai-delta` events and can be cancelled. `useAi` keeps one request's
// state for a component.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, inApp, onAiDelta } from "./api";
import type { AiOutcome, AiRequest, Destination } from "./types";

const handlers = new Map<string, (text: string) => void>();
let listening: Promise<void> | null = null;

/** One listener for every request, registered before the first one starts. */
function listen(): Promise<void> {
  if (!listening) {
    listening = new Promise((resolve) => {
      if (!inApp()) return resolve();
      onAiDelta((d) => handlers.get(d.id)?.(d.text));
      // The listener is set up asynchronously; a short wait covers it before the first request.
      setTimeout(resolve, 50);
    });
  }
  return listening;
}

let counter = 0;

/** The user's offset from UTC in seconds (the dates in prompts). */
export const utcOffset = () => -new Date().getTimezoneOffset() * 60;

/** Runs a request; `onText` gets the answer as it grows. */
export async function runAi(request: AiRequest, onText: (text: string) => void, onId?: (id: string) => void): Promise<AiOutcome> {
  await listen();
  const id = `ai-${Date.now()}-${++counter}`;
  onId?.(id);
  let text = "";
  handlers.set(id, (piece) => {
    text += piece;
    onText(text);
  });
  try {
    return await api.aiRun(id, request, utcOffset());
  } finally {
    handlers.delete(id);
  }
}

export type AiState = { text: string; busy: boolean; error: string | null; outcome: AiOutcome | null };

const IDLE: AiState = { text: "", busy: false, error: null, outcome: null };

/** One request at a time for a component: run, cancel, reset. */
export function useAi() {
  const [state, setState] = useState<AiState>(IDLE);
  const current = useRef<string | null>(null);
  const alive = useRef(true);
  useEffect(
    () => () => {
      alive.current = false;
      if (current.current) void api.aiCancel(current.current);
    },
    [],
  );
  const run = useCallback(async (request: AiRequest) => {
    if (current.current) void api.aiCancel(current.current);
    setState({ text: "", busy: true, error: null, outcome: null });
    let mine: string | null = null;
    try {
      const outcome = await runAi(
        request,
        (text) => {
          if (alive.current && current.current === mine) setState((s) => ({ ...s, text }));
        },
        (id) => {
          mine = id;
          current.current = id;
        },
      );
      if (alive.current && current.current === mine) setState({ text: outcome.text, busy: false, error: null, outcome });
      return outcome;
    } catch (e) {
      if (alive.current && current.current === mine) setState((s) => ({ ...s, busy: false, error: String(e) }));
      return null;
    } finally {
      if (current.current === mine) current.current = null;
    }
  }, []);
  const cancel = useCallback(() => {
    if (current.current) void api.aiCancel(current.current);
  }, []);
  const reset = useCallback(() => {
    if (current.current) void api.aiCancel(current.current);
    current.current = null;
    setState(IDLE);
  }, []);
  return { state, run, cancel, reset };
}

/** The privacy line's words: where the text goes. */
export function destinationKey(d: Destination | null): "ai.local" | "ai.cloud" | "ai.none" {
  if (!d || !d.provider) return "ai.none";
  return d.local ? "ai.local" : "ai.cloud";
}

/** `[n]` references of an answer, as text and reference parts. */
export function citeParts(text: string): ({ text: string } | { ref: number })[] {
  const out: ({ text: string } | { ref: number })[] = [];
  const re = /\[(\d+(?:\s*,\s*\d+)*)\]/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    for (const n of m[1].split(",")) out.push({ ref: Number(n.trim()) });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
