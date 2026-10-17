// What the update notice shows, and the release notes as blocks (src/components/Updates.tsx).
// The backend decides what is offered (src-tauri/src/updates.rs); this decides how it is told,
// as Arcalo does: in the mode „Automatisch“ only a downloaded update speaks up („Update bereit –
// wird beim Beenden installiert“), in the mode „Nur benachrichtigen“ every new version does, and
// a copy that cannot install itself says why and offers the release page.

import type { ApkStatus, ManualUpdate, UpdateStatus } from "./types";

/** The first automatic check after the start, then every few hours. */
export const FIRST_CHECK_DELAY_MS = 30_000;
export const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

/** „Später erinnern“: tomorrow or in a week. */
export const REMIND_DAYS = { tomorrow: 1, nextWeek: 7 } as const;

export type Notice =
  | { kind: "none" }
  /** Downloaded and verified: installs on quit, or now with „Jetzt neu starten“. */
  | { kind: "ready"; version: string }
  /** A new version to install with a click (mode „Nur benachrichtigen“), or to download by hand. */
  | { kind: "available"; version: string; manual: ManualUpdate | null };

/** The notice for `status`; `dismissed` is the version whose notice was closed this session. */
export function updateNotice(status: UpdateStatus | null, dismissed: string | null): Notice {
  const offer = status?.enabled ? status.available : null;
  if (!status || !offer || dismissed === offer.version) return { kind: "none" };
  if (status.manual) return { kind: "available", version: offer.version, manual: status.manual };
  if (status.mode === "notify") return { kind: "available", version: offer.version, manual: null };
  return status.ready === offer.version ? { kind: "ready", version: offer.version } : { kind: "none" };
}

/** „Neu in Arcalo Mail“ after an update: the version and its notes, once. */
export function whatsNew(status: UpdateStatus | null): { version: string; notes: string; url: string } | null {
  const r = status?.restarted;
  if (!r?.installed || !r.notes || status?.whats_new_seen === r.version) return null;
  return { version: r.version, notes: r.notes, url: r.url };
}

/** The Android app's notice: a newer APK to download, or one that cannot replace this app. */
export function apkNotice(status: ApkStatus | null, dismissed: string | null): ApkStatus["offered"] | null {
  if (!status?.enabled || !status.offered || dismissed === status.offered.version) return null;
  return status.phase === "available" || status.phase === "blocked" ? status.offered : null;
}

/** Bytes as MB with one decimal (the download's size). */
export function megabytes(bytes: number | null | undefined, locale: string): string | null {
  if (bytes == null || !Number.isFinite(bytes)) return null;
  return `${(bytes / 1_000_000).toLocaleString(locale, { maximumFractionDigits: 1, minimumFractionDigits: 1 })} MB`;
}

export type NotesBlock = { kind: "heading" | "paragraph"; text: string } | { kind: "list"; items: string[] };

/** Release notes (the Markdown of docs/releases) as headings, paragraphs and lists, with the
 *  inline marks (`**`, `` ` ``, links) reduced to their text: shown as plain React text, never as
 *  HTML. */
export function notesBlocks(markdown: string): NotesBlock[] {
  const plain = (s: string) =>
    s
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\*\*([^*]+)\*\*/g, "$1")
      .replace(/__([^_]+)__/g, "$1")
      .replace(/`([^`]+)`/g, "$1")
      .trim();
  const out: NotesBlock[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push({ kind: "paragraph", text: plain(para.join(" ")) });
    para = [];
  };
  for (const raw of markdown.replace(/\r\n/g, "\n").split("\n")) {
    const line = raw.trim();
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    const item = /^(?:[-*+]|\d+\.)\s+(?:\[[ xX]\]\s+)?(.*)$/.exec(line);
    if (!line) flush();
    else if (heading) {
      flush();
      out.push({ kind: "heading", text: plain(heading[1]) });
    } else if (item) {
      flush();
      const last = out[out.length - 1];
      if (last?.kind === "list") last.items.push(plain(item[1]));
      else out.push({ kind: "list", items: [plain(item[1])] });
    } else if (/^\s{2,}\S/.test(raw) && out[out.length - 1]?.kind === "list" && !para.length) {
      // The continuation line of a list item.
      const list = out[out.length - 1] as { kind: "list"; items: string[] };
      list.items[list.items.length - 1] += ` ${plain(line)}`;
    } else para.push(line);
  }
  flush();
  return out;
}
