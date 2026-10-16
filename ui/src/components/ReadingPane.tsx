import { Archive, ChevronLeft, Forward, ImageOff, Mail, MailOpen, Reply, ReplyAll, Trash2 } from "lucide-react";
import { currentLang, useT } from "../lib/i18n";
import { addressLine, displayName, fullDate } from "../lib/format";
import type { Theme } from "../lib/theme";
import type { MessageView } from "../lib/types";
import { MailFrame } from "./MailFrame";
import type { ComposeMode } from "../lib/compose";

type Props = {
  message: MessageView | null;
  threadCount: number;
  theme: Theme;
  onBack: () => void;
  onCompose: (mode: Exclude<ComposeMode, "new">) => void;
  onToggleSeen: () => void;
  onAllowRemote: () => void;
};

export function ReadingPane({ message, threadCount, theme, onBack, onCompose, onToggleSeen, onAllowRemote }: Props) {
  const t = useT();
  const lang = currentLang();
  if (!message) {
    return (
      <section className="reading-pane reading-empty" aria-label={t("read.label")}>
        <Mail size={32} strokeWidth={1.5} aria-hidden />
        <p className="empty-title">{t("read.none")}</p>
        <p className="muted">{t("read.noneHint")}</p>
      </section>
    );
  }
  const m = message.summary;
  const blocked = message.body.blocked_remote;
  return (
    <section className="reading-pane" aria-label={t("read.label")}>
      <div className="toolbar" role="toolbar" aria-label={t("read.actions")}>
        <button type="button" className="icon-btn back-btn" onClick={onBack} aria-label={t("common.back")}>
          <ChevronLeft size={18} strokeWidth={1.75} aria-hidden />
        </button>
        <button type="button" className="btn" onClick={() => onCompose("reply")}>
          <Reply size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("read.reply")}</span>
        </button>
        <button type="button" className="btn" onClick={() => onCompose("replyAll")}>
          <ReplyAll size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("read.replyAll")}</span>
        </button>
        <button type="button" className="btn" onClick={() => onCompose("forward")}>
          <Forward size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("read.forward")}</span>
        </button>
        <span className="toolbar-gap" />
        <button type="button" className="icon-btn" onClick={onToggleSeen} title={m.seen ? t("read.markUnread") : t("read.markRead")}>
          {m.seen ? <Mail size={16} strokeWidth={1.75} aria-hidden /> : <MailOpen size={16} strokeWidth={1.75} aria-hidden />}
        </button>
        <button type="button" className="icon-btn" disabled title={t("read.archive")}>
          <Archive size={16} strokeWidth={1.75} aria-hidden />
        </button>
        <button type="button" className="icon-btn" disabled title={t("read.delete")}>
          <Trash2 size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      <div className="reading-scroll">
        <header className="message-head">
          <h1 className="message-subject">{m.subject || t("list.noSubject")}</h1>
          {threadCount > 1 && <p className="muted">{t("read.thread", { n: threadCount })}</p>}
          <dl className="message-meta">
            <dt>{t("read.from")}</dt>
            <dd>
              <strong>{displayName(m.from, lang)}</strong>
              {m.from?.name && <span className="muted"> &lt;{m.from.email}&gt;</span>}
            </dd>
            <dt>{t("read.to")}</dt>
            <dd>{addressLine(message.to)}</dd>
            {message.cc.length > 0 && (
              <>
                <dt>{t("read.cc")}</dt>
                <dd>{addressLine(message.cc)}</dd>
              </>
            )}
          </dl>
          <time className="muted message-date" dateTime={new Date(m.date * 1000).toISOString()}>
            {fullDate(m.date, lang)}
          </time>
        </header>
        {blocked > 0 && !message.remote_allowed && (
          <div className="notice" role="status">
            <ImageOff size={16} strokeWidth={1.75} aria-hidden />
            <span>{t("read.remoteBlocked", { n: blocked })}</span>
            <button type="button" className="btn btn-sm" onClick={onAllowRemote}>
              {t("read.loadRemote")}
            </button>
          </div>
        )}
        <MailFrame html={message.body.html} allowRemote={message.remote_allowed} theme={theme} />
      </div>
    </section>
  );
}
