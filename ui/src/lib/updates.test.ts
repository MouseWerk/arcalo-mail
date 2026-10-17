import { describe, expect, it } from "vitest";
import { apkNotice, megabytes, notesBlocks, updateNotice, whatsNew } from "./updates";
import type { ApkStatus, UpdateStatus } from "./types";

const status = (over: Partial<UpdateStatus> = {}): UpdateStatus => ({
  enabled: true,
  current_version: "1.0.0",
  available: { version: "1.0.1", notes: "## Improved\n\n- Faster", date: "2026-11-02", url: "https://github.com/MouseWerk/arcalo-mail/releases/tag/v1.0.1" },
  manual: null,
  manual_text: null,
  restarted: null,
  mode: "auto",
  skipped: null,
  remind_after: null,
  whats_new_seen: null,
  download: { phase: "idle", downloaded: 0, total: null, percent: null, error: null },
  ready: null,
  waiting: null,
  release_page: "https://github.com/MouseWerk/arcalo-mail/releases/latest",
  ...over,
});

describe("the update notice", () => {
  it("stays quiet while the automatic mode downloads, and speaks once the update is ready", () => {
    expect(updateNotice(status(), null)).toEqual({ kind: "none" });
    expect(updateNotice(status({ download: { phase: "downloading", downloaded: 5, total: 10, percent: 50, error: null } }), null)).toEqual({ kind: "none" });
    expect(updateNotice(status({ ready: "1.0.1" }), null)).toEqual({ kind: "ready", version: "1.0.1" });
    expect(updateNotice(status({ ready: "1.0.0" }), null)).toEqual({ kind: "none" });
  });

  it("tells about every new version in the mode „Nur benachrichtigen“", () => {
    expect(updateNotice(status({ mode: "notify" }), null)).toEqual({ kind: "available", version: "1.0.1", manual: null });
  });

  it("offers the release page to a copy that cannot install itself", () => {
    expect(updateNotice(status({ manual: "portable" }), null)).toEqual({ kind: "available", version: "1.0.1", manual: "portable" });
    expect(updateNotice(status({ manual: "translocated", ready: "1.0.1" }), null)).toMatchObject({ kind: "available", manual: "translocated" });
  });

  it("is gone once closed, and in builds without an update key", () => {
    expect(updateNotice(status({ ready: "1.0.1" }), "1.0.1")).toEqual({ kind: "none" });
    expect(updateNotice(status({ ready: "1.0.2", available: { version: "1.0.2", notes: null, date: null, url: "" } }), "1.0.1")).toMatchObject({ kind: "ready" });
    expect(updateNotice(status({ enabled: false, ready: "1.0.1" }), null)).toEqual({ kind: "none" });
    expect(updateNotice(null, null)).toEqual({ kind: "none" });
  });
});

describe("„Neu in Arcalo Mail“", () => {
  const restarted = { version: "1.0.1", installed: true, from: "1.0.0", notes: "- Faster", url: "u" };
  it("shows the notes once after an update", () => {
    expect(whatsNew(status({ restarted }))).toEqual({ version: "1.0.1", notes: "- Faster", url: "u" });
    expect(whatsNew(status({ restarted, whats_new_seen: "1.0.1" }))).toBeNull();
    expect(whatsNew(status({ restarted: { ...restarted, installed: false } }))).toBeNull();
    expect(whatsNew(status({ restarted: { ...restarted, notes: null } }))).toBeNull();
    expect(whatsNew(status())).toBeNull();
  });
});

describe("the Android notice", () => {
  const apk = (over: Partial<ApkStatus> = {}): ApkStatus => ({
    enabled: true,
    store: false,
    current: "1.0.0",
    phase: "available",
    offered: { version: "1.0.1", notes: null, size: 40_000_000, pub_date: null, apk: "https://x/a.apk", page: "https://x" },
    error: null,
    blocked: null,
    last_check: null,
    updated_from: null,
    notes: null,
    ...over,
  });
  it("shows a newer APK, never for the Play build", () => {
    expect(apkNotice(apk(), null)?.version).toBe("1.0.1");
    expect(apkNotice(apk({ phase: "blocked" }), null)?.version).toBe("1.0.1");
    expect(apkNotice(apk({ enabled: false, store: true }), null)).toBeNull();
    expect(apkNotice(apk(), "1.0.1")).toBeNull();
    expect(apkNotice(apk({ phase: "checking" }), null)).toBeNull();
  });
  it("names the size in MB", () => {
    expect(megabytes(40_000_000, "en")).toBe("40.0 MB");
    expect(megabytes(41_250_000, "de")).toBe("41,3 MB");
    expect(megabytes(null, "en")).toBeNull();
  });
});

describe("release notes", () => {
  it("become headings, lists and paragraphs of plain text", () => {
    const md = "## Arcalo Mail 1.0.1\n\nA **calm** release.\nSecond line.\n\n## Improved\n\n- Search is `faster`.\n- A [link](https://x) stays text\n  across two lines.\n* [ ] a checkbox item\n\n1. numbered";
    expect(notesBlocks(md)).toEqual([
      { kind: "heading", text: "Arcalo Mail 1.0.1" },
      { kind: "paragraph", text: "A calm release. Second line." },
      { kind: "heading", text: "Improved" },
      { kind: "list", items: ["Search is faster.", "A link stays text across two lines.", "a checkbox item", "numbered"] },
    ]);
  });

  it("keeps markup as text, never as HTML", () => {
    expect(notesBlocks("<img src=x onerror=alert(1)>")).toEqual([{ kind: "paragraph", text: "<img src=x onerror=alert(1)>" }]);
  });
});
