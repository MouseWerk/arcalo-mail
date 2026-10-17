// The menus and the dialog of the time tools: snooze presets, „Später senden“, „Erinnern, wenn
// keine Antwort“, and the dialog that picks a date and time.

import { AlarmClock, BellOff, CalendarClock, Clock, Info, Sun, Sunrise } from "lucide-react";
import { useState } from "react";
import { currentLang, useT } from "../lib/i18n";
import { now } from "../lib/format";
import { FOLLOWUP_DAYS, fromLocalInput, sendPresets, snoozePresets, toLocalInput, whenLabel, type SendPreset, type SnoozePreset } from "../lib/when";
import { Dialog, type MenuItem } from "./ui";

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
      <label className="stack-field">
        <span className="field-label">{t("when.label")}</span>
        <input
          className="input when-input"
          type="datetime-local"
          value={value}
          min={toLocalInput(now().getTime() / 1000)}
          autoFocus
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
