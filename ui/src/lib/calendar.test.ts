// The calendar view's logic: ranges, the overlap layout, moving and resizing, the edited times.

import { describe, expect, it } from "vitest";
import { attendeeName, blockFit, parseDayText, parseTimeText, editedTimes, isoDay, keyAction, layoutDay, monthCells, moved, resized, step, viewRange } from "./calendar";
import type { CalendarEvent } from "./types";

const ev = (key: string, start: string, end: string, extra: Partial<CalendarEvent> = {}): CalendarEvent => ({
  key,
  calendar_id: 1,
  object_id: 1,
  uid: key,
  instance: "",
  title: key,
  start: new Date(start).toISOString(),
  end: new Date(end).toISOString(),
  all_day: false,
  location: "",
  organizer: "",
  attendees: [],
  body: null,
  link: null,
  link_kind: null,
  busy: "busy",
  private: false,
  recurring: false,
  categories: [],
  editable: "yes",
  write: null,
  reanswer: false,
  ...extra,
});

describe("calendar", () => {
  it("shows the work week's days and keeps today on a day off", () => {
    const wed = new Date(2026, 9, 14, 12);
    const r = viewRange("workweek", wed, 1, [1, 2, 3, 4, 5], wed);
    expect(r.days.map(isoDay)).toEqual(["2026-10-12", "2026-10-13", "2026-10-14", "2026-10-15", "2026-10-16"]);
    expect(isoDay(r.to)).toBe("2026-10-17");
    const sat = new Date(2026, 9, 17, 12);
    expect(viewRange("workweek", sat, 1, [1, 2, 3, 4, 5], sat).days.map(isoDay)).toContain("2026-10-17");
    expect(viewRange("month", wed, 1).days).toHaveLength(42);
    expect(isoDay(step("month", new Date(2026, 0, 31), 1))).toBe("2026-02-28");
  });

  it("lays overlapping appointments side by side", () => {
    const day = new Date(2026, 9, 14);
    const placed = layoutDay(
      [ev("a", "2026-10-14T09:00", "2026-10-14T10:00"), ev("b", "2026-10-14T09:30", "2026-10-14T11:00"), ev("c", "2026-10-14T12:00", "2026-10-14T12:10")],
      day,
    );
    const by = Object.fromEntries(placed.map((p) => [p.item.key, p]));
    expect([by.a.col, by.a.cols, by.b.col, by.b.cols]).toEqual([0, 2, 1, 2]);
    expect(by.c.cols).toBe(1);
    expect(by.c.height).toBe(20);
    expect(by.a.top).toBe(9 * 60);
  });

  it("moves and resizes on the quarter-hour grid", () => {
    const e = ev("a", "2026-10-14T09:00", "2026-10-14T10:00");
    const m = moved(e, 22, 1);
    expect(new Date(m.start).getDate()).toBe(15);
    expect(new Date(m.start).getMinutes()).toBe(15);
    expect(new Date(m.end).getTime() - new Date(m.start).getTime()).toBe(3600e3);
    const r = resized(e, -120);
    expect(new Date(r.end).getTime() - new Date(r.start).getTime()).toBe(15 * 60e3);
  });

  it("reads the panel's date and time fields", () => {
    const s = new Date(2026, 9, 14, 9, 0);
    const e = new Date(2026, 9, 14, 10, 0);
    expect(editedTimes(s, e, { date: "2026-10-14", from: "09:00", to: "10:00" })).toBeNull();
    const later = editedTimes(s, e, { date: "2026-10-14", from: "11:00", to: "10:00" })!;
    expect([later.start.getHours(), later.end.getHours()]).toEqual([11, 12]);
    const longer = editedTimes(s, e, { date: "2026-10-14", from: "09:00", to: "11:30" })!;
    expect(longer.end.getHours() * 60 + longer.end.getMinutes()).toBe(11 * 60 + 30);
    const overnight = editedTimes(s, e, { date: "2026-10-14", from: "09:00", to: "08:00" })!;
    expect(overnight.end.getDate()).toBe(15);
    expect(editedTimes(s, e, { date: "2026-10-14", from: "9", to: "10:00" })).toBeNull();
  });

  it("fills month cells with a „more“ line", () => {
    const d = new Date(2026, 9, 14);
    const list = [1, 2, 3, 4, 5].map((i) => ev(`e${i}`, `2026-10-14T0${i}:00`, `2026-10-14T0${i}:30`));
    const cell = monthCells([...list, ev("all", "2026-10-14T00:00", "2026-10-15T00:00", { all_day: true })], [d], 4).get("2026-10-14")!;
    expect(cell.shown.map((e) => e.key)).toEqual(["all", "e1", "e2"]);
    expect(cell.more).toBe(3);
  });

  it("fits short appointments on one line and names attendees", () => {
    expect(blockFit(22, 30, true).oneLine).toBe(true);
    expect(blockFit(94, 120, true)).toEqual({ oneLine: false, titleLines: 3, place: true });
    expect(attendeeName("Anna Müller <anna@example.org>")).toBe("Anna Müller");
    expect(attendeeName("anna@example.org")).toBe("anna@example.org");
    expect(keyAction("a")).toEqual({ view: "workweek" });
    expect(keyAction("x")).toBeNull();
  });

  it("reads typed days and times", () => {
    expect(parseDayText("17.10.2026")).toBe("2026-10-17");
    expect(parseDayText("1/2/26")).toBe("2026-02-01");
    expect(parseDayText("2026-10-17")).toBe("2026-10-17");
    expect(parseDayText("31.02.2026")).toBeNull();
    expect(parseTimeText("9")).toBe("09:00");
    expect(parseTimeText("930")).toBe("09:30");
    expect(parseTimeText("14.15")).toBe("14:15");
    expect(parseTimeText("25:00")).toBeNull();
  });
});
