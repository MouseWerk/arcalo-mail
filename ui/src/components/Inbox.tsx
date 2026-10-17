// The sorted inbox's parts of the list pane: the tabs with their unread counts, the focus note,
// and the calm state when everything is done.

import { Bell, CircleCheck, Crosshair, Newspaper, Receipt, Shapes, UsersRound } from "lucide-react";
import { useT, type TKey } from "../lib/i18n";
import type { Category, InboxCounts } from "../lib/types";

export const CATEGORIES: Category[] = ["people", "newsletters", "notifications", "receipts", "other"];

export const CATEGORY_LABEL: Record<Category, TKey> = {
  people: "cat.people",
  newsletters: "cat.newsletters",
  notifications: "cat.notifications",
  receipts: "cat.receipts",
  other: "cat.other",
};

export const CATEGORY_ICON: Record<Category, typeof Bell> = {
  people: UsersRound,
  newsletters: Newspaper,
  notifications: Bell,
  receipts: Receipt,
  other: Shapes,
};

/** The tabs (a tab without mail and not chosen stays out, People always shows), or the focus
 *  note while focus is on. */
export function InboxHead({ tab, counts, focus, onTab, onFocusEnd }: { tab: Category; counts: InboxCounts | null; focus: boolean; onTab: (c: Category) => void; onFocusEnd: () => void }) {
  const t = useT();
  if (focus)
    return (
      <div className="focus-note" role="status">
        <Crosshair size={14} strokeWidth={1.75} aria-hidden />
        <span>{t("inbox.focusOn")}</span>
        <button type="button" className="link-btn" onClick={onFocusEnd}>
          {t("inbox.focusEnd")}
        </button>
      </div>
    );
  const shown = CATEGORIES.filter((c) => c === "people" || c === tab || (counts?.tabs.find((x) => x.category === c)?.total ?? 0) > 0);
  return (
    <div className="inbox-tabs" role="tablist" aria-label={t("inbox.tabs")}>
      {shown.map((c) => {
        const count = counts?.tabs.find((x) => x.category === c);
        const Icon = CATEGORY_ICON[c];
        return (
          <button
            key={c}
            type="button"
            role="tab"
            data-tab={c}
            aria-selected={c === tab}
            className="inbox-tab"
            onClick={() => onTab(c)}
            onKeyDown={(e) => {
              if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
              e.preventDefault();
              const i = shown.indexOf(c) + (e.key === "ArrowRight" ? 1 : -1);
              const next = shown[(i + shown.length) % shown.length];
              onTab(next);
              (e.currentTarget.parentElement?.querySelector(`[data-tab="${next}"]`) as HTMLElement | null)?.focus();
            }}
            tabIndex={c === tab ? 0 : -1}
            aria-label={count && count.unread > 0 ? `${t(CATEGORY_LABEL[c])}, ${t("nav.unread", { n: count.unread })}` : t(CATEGORY_LABEL[c])}
            title={t(CATEGORY_LABEL[c])}
          >
            <Icon size={14} strokeWidth={1.75} aria-hidden />
            <span className="inbox-tab-label">{t(CATEGORY_LABEL[c])}</span>
            {count && count.unread > 0 && (
              <span className="inbox-tab-count" aria-hidden>
                {count.unread}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** Nothing left in the tab: for People and Focus the calm inbox zero, with the other tabs that
 *  still hold unread mail and the snoozed messages that come back. */
export function InboxZero({ tab, focus, counts, snoozed, onTab }: { tab: Category; focus: boolean; counts: InboxCounts | null; snoozed: number; onTab: (c: Category) => void }) {
  const t = useT();
  if (!focus && tab !== "people") {
    const Icon = CATEGORY_ICON[tab];
    return (
      <div className="empty">
        <Icon size={28} strokeWidth={1.5} aria-hidden />
        <p>{t("inbox.emptyTab", { name: t(CATEGORY_LABEL[tab]) })}</p>
      </div>
    );
  }
  const others = focus ? [] : (counts?.tabs ?? []).filter((c) => c.category !== "people" && c.unread > 0);
  return (
    <div className="inbox-zero" role="status">
      <span className="inbox-zero-mark" aria-hidden>
        <CircleCheck size={30} strokeWidth={1.5} />
      </span>
      <h2>{t("inbox.zeroTitle")}</h2>
      <p className="muted">{t("inbox.zeroText")}</p>
      {others.length > 0 && (
        <div className="inbox-zero-more">
          <span className="muted">{t("inbox.zeroMore")}</span>
          <div className="inbox-zero-chips">
            {others.map((c) => {
              const Icon = CATEGORY_ICON[c.category];
              return (
                <button key={c.category} type="button" className="chip-btn" onClick={() => onTab(c.category)}>
                  <Icon size={13} strokeWidth={1.75} aria-hidden />
                  <span>{t(CATEGORY_LABEL[c.category])}</span>
                  <span className="inbox-tab-count">{c.unread}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
      {snoozed > 0 && <p className="muted inbox-zero-snoozed">{t("inbox.zeroSnoozed", { n: snoozed })}</p>}
    </div>
  );
}
