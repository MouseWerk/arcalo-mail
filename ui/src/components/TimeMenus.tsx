// The menus and the dialog of the time tools: snooze presets, „Später senden“, „Erinnern, wenn
// keine Antwort“, and the dialog that picks a date and time.

import { AlarmClock, BellOff, CalendarClock, ChevronLeft, ChevronRight, Clock, Info, Sun, Sunrise } from "lucide-react";
import { useState } from "react";
import { currentLang, useT } from "../lib/i18n";
import { now } from "../lib/format";
import { FOLLOWUP_DAYS, fromLocalInput, sendPresets, snoozePresets, toLocalInput, whenLabel, type SendPreset, type SnoozePreset } from "../lib/when";
import { addMonths, dateLocale, isoDay, monthGrid, weekStartsOn } from "../lib/calendar";
import { Dialog, Select, type MenuItem } from "./ui";

const SNOOZE_ICON: Record<SnoozePreset, typeof Clock> = { laterToday: Clock, tomorrow: Sunrise, weekend: Sun, nextWeek: CalendarClock };
const SEND_ICON: Record<SendPreset, typeof Clock> = { tomorrowMorning: Sunrise, tomorrowAfternoon: Sun, nextWeek: CalendarClock };

/** „Zurückstellen“: the presets with their times, and „Zeitpunkt wählen…“. */
export function snoozeItems(t: ReturnType<typeof useT>, onPick: (at: number) => void, onCustom: () => void): MenuItem[] {
  const lang = currentLang();
  const n = now();
  return [
    { heading: t("snooze.title") },
    ...snoozePresets(n).map((p): MenuItem => {
      const Icon = SNOOZE_ICON[p.id];
      return { label: t(`snooze.${p.id}`), hint: whenLabel(p.at, n, lang), icon: <Icon size={14} aria-hidden />, onSelect: () => onPick(p.at) };
    }),
    "separator",
    { label: t("when.custom"), icon: <CalendarClock size={14} aria-hidden />, onSelect: onCustom },
  ];
}

/** „Später senden“: tomorrow morning and afternoon, next Monday, and „Zeitpunkt wählen…“. */
export function sendLaterItems(t: ReturnType<typeof useT>, onPick: (at: number) => void, onCustom: () => void): MenuItem[] {
  const lang = currentLang();
  const n = now();
  return [
    { heading: t("compose.sendLater") },
    ...sendPresets(n).map((p): MenuItem => {
      const Icon = SEND_ICON[p.id];
      return { label: t(`send.${p.id}`), hint: whenLabel(p.at, n, lang), icon: <Icon size={14} aria-hidden />, onSelect: () => onPick(p.at) };
    }),
    "separator",
    { label: t("when.custom"), icon: <CalendarClock size={14} aria-hidden />, onSelect: onCustom },
  ];
}

/** „Erinnern, wenn keine Antwort“: after one, three or seven days (`days`: the one in effect),
 *  „Zeitpunkt wählen…“ and „Keine Erinnerung“. */
export function followupItems(
  t: ReturnType<typeof useT>,
  days: number | null,
  onPick: (days: number) => void,
  onCustom: (() => void) | null,
  onNone: (() => void) | null,
): MenuItem[] {
  const lang = currentLang();
  const n = now();
  const items: MenuItem[] = [{ heading: t("compose.remind") }];
  for (const d of FOLLOWUP_DAYS) {
    items.push({
      label: t("followup.after", { n: d }),
      hint: whenLabel(n.getTime() / 1000 + d * 86400, n, lang),
      icon: <AlarmClock size={14} aria-hidden />,
      checked: days === null ? undefined : days === d,
      onSelect: () => onPick(d),
    });
  }
  if (onCustom) items.push({ label: t("when.custom"), icon: <CalendarClock size={14} aria-hidden />, onSelect: onCustom });
  if (onNone) items.push("separator", { label: t("read.followupOff"), icon: <BellOff size={14} aria-hidden />, onSelect: onNone });
  return items;
}

/** Picks a date and time in the future (the custom snooze, send-later or reminder time). */
export function WhenDialog({
  title,
  confirm,
  note,
  initial,
  onPick,
  onClose,
}: {
  title: string;
  confirm: string;
  note?: string;
  /** Unix seconds; by default tomorrow at 08:00. */
  initial?: number;
  onPick: (at: number) => void;
  onClose: () => void;
}) {
  const t = useT();
  const start = initial ?? snoozePresets(now()).find((p) => p.id === "tomorrow")!.at;
  const [value, setValue] = useState(toLocalInput(start));
  const at = fromLocalInput(value);
  const past = at !== null && at <= now().getTime() / 1000;
  const submit = () => {
    if (at === null || past) return;
    onPick(at);
    onClose();
  };
  return (
    <Dialog
      title={title}
      onClose={onClose}
      className="when-dialog"
      above
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={at === null || past} onClick={submit}>
            {confirm}
          </button>
        </>
      }
    >
      <DateTimePicker value={value} onChange={setValue} />
      <label className="stack-field when-typed">
        <span className="field-label">{t("when.label")}</span>
        <input
          className="input when-input"
          type="text"
          inputMode="numeric"
          enterKeyHint="done"
          autoComplete="off"
          spellCheck={false} autoCapitalize="off" autoCorrect="off"
          placeholder="2026-03-12T08:00"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
      </label>
      {at !== null && !past && (
        <p className="when-preview">
          <Clock size={14} strokeWidth={1.75} aria-hidden />
          <span>{whenLabel(at, now(), currentLang())}</span>
        </p>
      )}
      {past && (
        <p className="field-error" role="alert">
          {t("when.past")}
        </p>
      )}
      {note && (
        <p className="when-note">
          <Info size={14} strokeWidth={1.75} aria-hidden />
          <span>{note}</span>
        </p>
      )}
    </Dialog>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");

/**
 * A day and a time, drawn by the app (the system's date and time pickers do not match it): a
 * month to tap a day in, past days dimmed, and the hour and minute as the app's own selects.
 * `value` is in the form of a `datetime-local` field („2026-03-12T08:00“).
 */
export function DateTimePicker({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const t = useT();
  const at = fromLocalInput(value);
  const chosen = at === null ? now() : new Date(at * 1000);
  const [month, setMonth] = useState(() => new Date(chosen.getFullYear(), chosen.getMonth(), 1));
  const today = now();
  const startsOn = weekStartsOn();
  const days = monthGrid(month.getFullYear(), month.getMonth(), startsOn);
  const l = dateLocale();
  const set = (d: Date, h: number, m: number) => onChange(`${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(h)}:${pad(m)}`);
  const h = chosen.getHours();
  const m = chosen.getMinutes();
  const minutes = Array.from({ length: 12 }, (_, i) => i * 5);
  if (!minutes.includes(m)) minutes.push(m);
  minutes.sort((a, b) => a - b);
  const before = (d: Date) => isoDay(d) < isoDay(today);
  return (
    <div className="dt-picker">
      <div className="dt-month">
        <button type="button" className="icon-btn" aria-label={t("calv.back")} onClick={() => setMonth((x) => addMonths(x, -1))}>
          <ChevronLeft size={16} strokeWidth={1.75} aria-hidden />
        </button>
        <span className="dt-month-name">{month.toLocaleDateString(l, { month: "long", year: "numeric" })}</span>
        <button type="button" className="icon-btn" aria-label={t("calv.forward")} onClick={() => setMonth((x) => addMonths(x, 1))}>
          <ChevronRight size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      <div className="dt-grid" role="grid" aria-label={month.toLocaleDateString(l, { month: "long", year: "numeric" })}>
        {days.slice(0, 7).map((d) => (
          <span key={`h${d.getDay()}`} className="dt-weekday" aria-hidden>
            {d.toLocaleDateString(l, { weekday: "narrow" })}
          </span>
        ))}
        {days.map((d) => {
          const on = isoDay(d) === isoDay(chosen);
          return (
            <button
              key={isoDay(d)}
              type="button"
              className="dt-day"
              data-other={d.getMonth() !== month.getMonth() || undefined}
              data-today={isoDay(d) === isoDay(today) || undefined}
              aria-pressed={on}
              aria-label={d.toLocaleDateString(l, { weekday: "long", day: "numeric", month: "long" })}
              disabled={before(d)}
              onClick={() => set(d, h, m)}
            >
              {d.getDate()}
            </button>
          );
        })}
      </div>
      <div className="dt-time">
        <Clock size={14} strokeWidth={1.75} aria-hidden />
        <Select className="select dt-hour" aria-label={t("when.hour")} value={h} onChange={(e) => set(chosen, Number(e.target.value), m)}>
          {Array.from({ length: 24 }, (_, i) => (
            <option key={i} value={i}>
              {pad(i)}
            </option>
          ))}
        </Select>
        <span aria-hidden>:</span>
        <Select className="select dt-minute" aria-label={t("when.minute")} value={m} onChange={(e) => set(chosen, h, Number(e.target.value))}>
          {minutes.map((x) => (
            <option key={x} value={x}>
              {pad(x)}
            </option>
          ))}
        </Select>
      </div>
    </div>
  );
}
