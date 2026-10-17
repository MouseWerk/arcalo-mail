// The list pane's views that are not message lists: „Neue Absender“ (the Screener), the
// subscriptions with „Abbestellen“, and the messages scheduled with „Später senden“.

import { BellRing, Check, Clock, Mail, MailCheck, Menu as MenuIcon, Newspaper, Pencil, Send, ShieldOff, Trash2, UserCheck } from "lucide-react";
import type { ReactNode } from "react";
import { currentLang, useT } from "../lib/i18n";
import { initials, listDate, now } from "../lib/format";
import { whenLabel } from "../lib/when";
import type { AccountInfo, MessageSummary, Scheduled, Subscription, WaitingSender } from "../lib/types";

function Pane({ title, count, note, onShowFolders, children, className }: { title: string; count?: string; note: string; onShowFolders: () => void; children: ReactNode; className: string }) {
  const t = useT();
  return (
    <section className={`list-pane smart-pane ${className}`} aria-label={title}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{title}</h1>
          {count && <span className="muted list-count">{count}</span>}
        </div>
        <p className="muted smart-note">{note}</p>
      </header>
      {children}
    </section>
  );
}

function Empty({ icon, text }: { icon: ReactNode; text: string }) {
  return (
    <div className="empty smart-empty">
      {icon}
      <p>{text}</p>
    </div>
  );
}

/** „Neue Absender“: senders the Screener holds back, each with „Zulassen“ and „Abweisen“. */
export function ScreenerView({
  senders,
  current,
  onOpen,
  onAllow,
  onBlock,
  onShowFolders,
}: {
  senders: WaitingSender[];
  current: number | null;
  onOpen: (m: MessageSummary) => void;
  onAllow: (s: WaitingSender) => void;
  onBlock: (s: WaitingSender) => void;
  onShowFolders: () => void;
}) {
  const t = useT();
  const lang = currentLang();
  return (
    <Pane title={t("folder.screener")} count={senders.length ? String(senders.length) : undefined} note={t("screener.note")} onShowFolders={onShowFolders} className="screener">
      {senders.length === 0 ? (
        <Empty icon={<UserCheck size={28} strokeWidth={1.5} aria-hidden />} text={t("screener.empty")} />
      ) : (
        <ul className="smart-list">
          {senders.map((s) => {
            const newest = s.messages[0];
            const name = s.name || s.email;
            return (
              <li key={s.email} className="smart-item screener-item" aria-current={newest && s.messages.some((m) => m.id === current) ? "true" : undefined}>
                <button type="button" className="smart-main" onClick={() => newest && onOpen(newest)}>
                  <span className="avatar" aria-hidden>
                    {initials({ name: s.name, email: s.email })}
                  </span>
                  <span className="smart-text">
                    <span className="smart-title">
                      <span className="smart-name">{name}</span>
                      {s.name && <span className="smart-sub">{s.email}</span>}
                    </span>
                    {newest && (
                      <span className="smart-meta">
                        {s.messages.length > 1 && <span className="thread-count">{s.messages.length}</span>}
                        <span className="smart-subject">{newest.subject || t("list.noSubject")}</span>
                        <span className="smart-date">{listDate(newest.date, now(), lang)}</span>
                      </span>
                    )}
                  </span>
                </button>
                <div className="smart-actions">
                  <button type="button" className="btn btn-sm" onClick={() => onAllow(s)}>
                    <Check size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("screener.allow")}</span>
                  </button>
                  <button type="button" className="btn btn-sm btn-quiet" onClick={() => onBlock(s)}>
                    <ShieldOff size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("screener.block")}</span>
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Pane>
  );
}

const METHOD = { one_click: "subs.oneClick", mailto: "subs.byMail", link: "subs.byPage" } as const;

/** The senders with an unsubscribe address, and „Abbestellen“. */
export function SubscriptionsView({
  list,
  busy,
  onOpen,
  onUnsubscribe,
  onShowFolders,
}: {
  list: Subscription[];
  busy: string | null;
  onOpen: (s: Subscription) => void;
  onUnsubscribe: (s: Subscription) => void;
  onShowFolders: () => void;
}) {
  const t = useT();
  const lang = currentLang();
  return (
    <Pane title={t("folder.subscriptions")} count={list.length ? String(list.length) : undefined} note={t("subs.note")} onShowFolders={onShowFolders} className="subscriptions">
      {list.length === 0 ? (
        <Empty icon={<Newspaper size={28} strokeWidth={1.5} aria-hidden />} text={t("subs.empty")} />
      ) : (
        <ul className="smart-list">
          {list.map((s) => {
            const name = s.name || s.sender;
            return (
              <li key={s.sender} className="smart-item subscription-item" data-done={s.unsubscribed ? "" : undefined}>
                <button type="button" className="smart-main" onClick={() => onOpen(s)}>
                  <span className="avatar" aria-hidden>
                    {initials({ name: s.name, email: s.sender })}
                  </span>
                  <span className="smart-text">
                    <span className="smart-title">
                      <span className="smart-name">{name}</span>
                      {s.name && <span className="smart-sub">{s.sender}</span>}
                    </span>
                    <span className="smart-meta">
                      <span>{t("list.count", { n: s.count })}</span>
                      <span aria-hidden>·</span>
                      <span>{t("subs.lastOn", { date: listDate(s.last, now(), lang) })}</span>
                      {s.method && !s.unsubscribed && (
                        <>
                          <span aria-hidden>·</span>
                          <span>{t(METHOD[s.method])}</span>
                        </>
                      )}
                    </span>
                  </span>
                </button>
                <div className="smart-actions">
                  {s.unsubscribed ? (
                    <span className="smart-badge">
                      <MailCheck size={14} strokeWidth={1.75} aria-hidden />
                      <span>{t("subs.unsubscribed")}</span>
                    </span>
                  ) : (
                    <button type="button" className="btn btn-sm" disabled={!s.method || busy === s.sender} onClick={() => onUnsubscribe(s)}>
                      <Mail size={14} strokeWidth={1.75} aria-hidden />
                      <span>{t("subs.unsubscribe")}</span>
                    </button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Pane>
  );
}

/** Messages waiting for „Später senden“. */
export function ScheduledView({
  list,
  accounts,
  mobile,
  onSendNow,
  onEdit,
  onDiscard,
  onShowFolders,
}: {
  list: Scheduled[];
  accounts: AccountInfo[];
  mobile: boolean;
  onSendNow: (s: Scheduled) => void;
  onEdit: (s: Scheduled) => void;
  onDiscard: (s: Scheduled) => void;
  onShowFolders: () => void;
}) {
  const t = useT();
  const lang = currentLang();
  return (
    <Pane
      title={t("folder.scheduled")}
      count={list.length ? String(list.length) : undefined}
      note={mobile ? t("scheduled.noteMobile") : t("scheduled.note")}
      onShowFolders={onShowFolders}
      className="scheduled"
    >
      {list.length === 0 ? (
        <Empty icon={<Clock size={28} strokeWidth={1.5} aria-hidden />} text={t("scheduled.empty")} />
      ) : (
        <ul className="smart-list">
          {list.map((s) => {
            const account = accounts.find((a) => a.id === s.account_id);
            return (
              <li key={s.id} className="smart-item scheduled-item">
                <div className="smart-main">
                  <span className="smart-icon" aria-hidden>
                    <Send size={16} strokeWidth={1.75} />
                  </span>
                  <span className="smart-text">
                    <span className="smart-title">
                      <span className="smart-name">{s.subject || t("list.noSubject")}</span>
                    </span>
                    <span className="smart-meta">
                      <span>{s.recipients}</span>
                      {accounts.length > 1 && account && (
                        <>
                          <span aria-hidden>·</span>
                          <span>{account.name || account.email}</span>
                        </>
                      )}
                    </span>
                    <span className="smart-when">
                      <Clock size={12} strokeWidth={1.75} aria-hidden />
                      <span>{t("scheduled.at", { when: whenLabel(s.due_at, now(), lang) })}</span>
                      {s.followup_secs != null && (
                        <>
                          <BellRing size={12} strokeWidth={1.75} aria-hidden />
                          <span>{t("scheduled.reminder")}</span>
                        </>
                      )}
                    </span>
                  </span>
                </div>
                <div className="smart-actions">
                  <button type="button" className="btn btn-sm" onClick={() => onSendNow(s)}>
                    <Send size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("scheduled.sendNow")}</span>
                  </button>
                  <button type="button" className="icon-btn icon-btn-sm" onClick={() => onEdit(s)} aria-label={t("scheduled.edit")} title={t("scheduled.edit")}>
                    <Pencil size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                  <button type="button" className="icon-btn icon-btn-sm" onClick={() => onDiscard(s)} aria-label={t("compose.discard")} title={t("compose.discard")}>
                    <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Pane>
  );
}
