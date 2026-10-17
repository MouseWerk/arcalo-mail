import {
  Archive,
  BellRing,
  CheckCheck,
  ChevronLeft,
  Clock,
  MailCheck,
  MailMinus,
  MoreHorizontal,
  Pin,
  ShieldCheck,
  Download,
  Eye,
  File,
  FileImage,
  FileText,
  Flag,
  FolderInput,
  Forward,
  ImageOff,
  Mail,
  MailOpen,
  Paperclip,
  Pencil,
  Reply,
  ReplyAll,
  ShieldAlert,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { useState } from "react";
import { currentLang, useT } from "../lib/i18n";
import { addressLine, displayName, fileSize, fullDate, now, shortDate } from "../lib/format";
import { whenLabel } from "../lib/when";
import type { Theme } from "../lib/theme";
import type { Attachment, MessageSummary, MessageView } from "../lib/types";
import { MailFrame } from "./MailFrame";
import { Spinner } from "./ui";
import { Avatar } from "./Avatar";
import { InvitationCard, JoinButton, ProtectedFiles, ProtectionBadges } from "./Meeting";
import type { Address } from "../lib/types";

export type Loaded = MessageView | "loading" | { error: string };

export type MessageAction = "archive" | "delete" | "spam" | "move" | "toggleSeen" | "flag" | "snooze" | "more";

/** What the notes on a message card take back: the snooze, the reminder, the pin. */
export type MarkAction = "unsnooze" | "removeFollowup" | "unpin";

type Props = {
  /** The conversation (or the one message), oldest first. */
  thread: MessageSummary[];
  views: Record<number, Loaded>;
  expanded: number[];
  /** The message the list points at. */
  focusId: number | null;
  theme: Theme;
  onToggle: (id: number) => void;
  onBack: () => void;
  onCompose: (mode: "reply" | "replyAll" | "forward", view: MessageView) => void;
  onAction: (a: MessageAction, anchor?: DOMRect) => void;
  onAllowRemote: (view: MessageView, always: boolean) => void;
  onAttachment: (a: Attachment, how: "open" | "save" | "preview") => void;
  onRetry: (id: number) => void;
  onEditDraft: (view: MessageView) => void;
  /** Touch screens: the actions in a bar at the bottom, in reach of the thumb. */
  touch?: boolean;
  /** The sorted inbox: archiving is „Erledigt“. */
  inbox?: boolean;
  /** A bundle opened as a stream of the sender's issues, newest first. */
  stream?: { name: string } | null;
  /** Above the conversation (a reminder that came due). */
  banner?: React.ReactNode;
  onUnsubscribe: (view: MessageView) => void;
  onMark: (a: MarkAction, view: MessageView) => void;
  /** The AI assistant above the conversation. */
  assistant?: React.ReactNode;
  /** A sender or recipient was clicked: the contact card. */
  onContact?: (a: Address) => void;
  /** The calendar on an invitation's appointment (or its day). */
  onOpenCalendar?: (key: string | null, start: string) => void;
};

function attachmentIcon(mime: string) {
  if (mime.startsWith("image/")) return FileImage;
  if (mime.startsWith("text/") || mime === "application/pdf") return FileText;
  return File;
}

function Attachments({ files, onAttachment }: { files: Attachment[]; onAttachment: Props["onAttachment"] }) {
  const t = useT();
  const lang = currentLang();
  return (
    <div className="attachments" aria-label={t("read.attachments", { n: files.length })} role="group">
      <div className="attachments-head">
        <Paperclip size={14} strokeWidth={1.75} aria-hidden />
        <span>{t("read.attachments", { n: files.length })}</span>
      </div>
      <ul className="attachment-list">
        {files.map((a) => {
          const Icon = attachmentIcon(a.mime_type);
          const name = a.filename ?? t("read.unnamed");
          const previewable = a.mime_type.startsWith("image/") && !a.mime_type.includes("svg");
          return (
            <li key={a.id} className="attachment">
              <button
                type="button"
                className="attachment-main"
                onClick={() => onAttachment(a, previewable ? "preview" : "open")}
                title={previewable ? t("read.preview") : t("read.open")}
              >
                <span className="attachment-icon">
                  <Icon size={18} strokeWidth={1.5} aria-hidden />
                </span>
                <span className="attachment-text">
                  <span className="attachment-name">{name}</span>
                  <span className="attachment-size">{fileSize(a.size, lang)}</span>
                </span>
              </button>
              {previewable && (
                <button type="button" className="icon-btn icon-btn-sm" onClick={() => onAttachment(a, "open")} aria-label={t("read.openNamed", { name })} title={t("read.open")}>
                  <Eye size={14} strokeWidth={1.75} aria-hidden />
                </button>
              )}
              <button type="button" className="icon-btn icon-btn-sm" onClick={() => onAttachment(a, "save")} aria-label={t("read.saveNamed", { name })} title={t("read.save")}>
                <Download size={14} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** The notes below a card's head: pinned, snoozed until, a reminder; each with its way back. */
function MarkNotes({ view, p }: { view: MessageView; p: Props }) {
  const t = useT();
  const lang = currentLang();
  const m = view.marks;
  if (!m || (!m.pinned && m.snoozed_until == null && m.followup_due == null)) return null;
  return (
    <div className="mark-notes">
      {m.pinned && (
        <span className="mark-note">
          <Pin size={13} strokeWidth={1.75} aria-hidden />
          <span>{t("read.pinned")}</span>
          <button type="button" className="link-btn" onClick={() => p.onMark("unpin", view)}>
            {t("read.unpin")}
          </button>
        </span>
      )}
      {m.snoozed_until != null && (
        <span className="mark-note">
          <Clock size={13} strokeWidth={1.75} aria-hidden />
          <span>{t("read.snoozedUntil", { when: whenLabel(m.snoozed_until, now(), lang) })}</span>
          <button type="button" className="link-btn" onClick={() => p.onMark("unsnooze", view)}>
            {t("snooze.unsnooze")}
          </button>
        </span>
      )}
      {m.followup_due != null && (
        <span className="mark-note">
          <BellRing size={13} strokeWidth={1.75} aria-hidden />
          <span>{t("read.followupAt", { when: whenLabel(m.followup_due, now(), lang) })}</span>
          <button type="button" className="link-btn" onClick={() => p.onMark("removeFollowup", view)}>
            {t("read.remove")}
          </button>
        </span>
      )}
    </div>
  );
}

function MessageCard({ view, theme, p, onCollapse }: { view: MessageView; theme: Theme; p: Props; onCollapse?: () => void }) {
  const t = useT();
  const lang = currentLang();
  const [showQuote, setShowQuote] = useState(false);
  const [asText, setAsText] = useState(false);
  const m = view.summary;
  const from = m.from;
  const blocked = view.body.blocked_remote;
  const html = asText && view.body.text_html ? view.body.text_html : showQuote && view.body.full_html ? view.body.full_html : view.body.html;
  const recipients = [...view.to, ...view.cc];
  return (
    <article className="message-card" aria-label={t("read.messageFrom", { name: displayName(from, lang) })}>
      <header
        className={`message-card-head${onCollapse ? " collapsible" : ""}`}
        onClick={(e) => {
          if (onCollapse && !(e.target as HTMLElement).closest("button")) onCollapse();
        }}
      >
        {from && p.onContact ? (
          <button type="button" className="avatar-btn" onClick={() => p.onContact?.(from)} aria-label={t("contact.open", { name: displayName(from, lang) })} title={t("contact.open", { name: displayName(from, lang) })}>
            <Avatar who={from} />
          </button>
        ) : (
          <Avatar who={from} />
        )}
        <div className="message-who">
          <div className="message-from">
            {from && p.onContact ? (
              <button type="button" className="link-btn sender-btn" onClick={() => p.onContact?.(from)}>
                <strong>{displayName(from, lang)}</strong>
              </button>
            ) : (
              <strong>{displayName(from, lang)}</strong>
            )}
            {from?.name && <span className="muted"> &lt;{from.email}&gt;</span>}
          </div>
          <div className="message-to muted" title={addressLine(recipients)}>
            {t("read.toLine", { names: recipients.map((a) => a.name || a.email).join(", ") || "–" })}
          </div>
        </div>
        <time className="muted message-date" dateTime={new Date(m.date * 1000).toISOString()} title={fullDate(m.date, lang)}>
          {shortDate(m.date, now(), lang)}
        </time>
        <div className="message-card-actions">
          {view.unsubscribe &&
            (view.unsubscribed ? (
              <span className="unsubscribed-note">
                <MailCheck size={14} strokeWidth={1.75} aria-hidden />
                <span>{t("subs.unsubscribed")}</span>
              </span>
            ) : (
              <button type="button" className="btn btn-sm btn-quiet unsubscribe-btn" onClick={() => p.onUnsubscribe(view)} title={t("read.unsubscribe")}>
                <MailMinus size={14} strokeWidth={1.75} aria-hidden />
                <span>{t("read.unsubscribe")}</span>
              </button>
            ))}
          <button type="button" className="icon-btn icon-btn-sm" onClick={() => p.onCompose("reply", view)} aria-label={t("read.reply")} title={t("read.reply")}>
            <Reply size={15} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
      </header>
      {p.stream && <h2 className="stream-subject">{m.subject || t("list.noSubject")}</h2>}
      <MarkNotes view={view} p={p} />
      {view.protection && <ProtectionBadges p={view.protection} />}
      {view.invitation && <InvitationCard view={view} onAnswered={() => p.onRetry(m.id)} onOpenCalendar={(key, start) => p.onOpenCalendar?.(key, start)} />}
      {!view.invitation && view.meeting && (
        <div className="meeting-row">
          <JoinButton url={view.meeting.url} kind={view.meeting.kind} />
        </div>
      )}
      {view.draft && (
        <div className="notice" role="status">
          <Pencil size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("read.isDraft")}</span>
          <button type="button" className="btn btn-sm" onClick={() => p.onEditDraft(view)}>
            {t("read.editDraft")}
          </button>
        </div>
      )}
      {view.load_error && (
        <div className="notice notice-warning" role="alert">
          <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
          <span>
            <strong>{t("read.loadFailed")}</strong> {view.load_error}
          </span>
          <button type="button" className="btn btn-sm" onClick={() => p.onRetry(m.id)}>
            {t("common.retry")}
          </button>
        </div>
      )}
      {blocked > 0 && !view.remote_allowed && (
        <div className="notice" role="status">
          <ImageOff size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("read.remoteBlocked", { n: blocked })}</span>
          <span className="notice-actions">
            <button type="button" className="btn btn-sm" onClick={() => p.onAllowRemote(view, false)}>
              {t("read.loadRemote")}
            </button>
            {from && (
              <button type="button" className="btn btn-sm btn-quiet" onClick={() => p.onAllowRemote(view, true)}>
                {t("read.alwaysFrom", { email: from.email })}
              </button>
            )}
          </span>
        </div>
      )}
      {view.body.trackers > 0 && (
        <p className="tracker-note" title={t("read.trackersHint")}>
          <ShieldCheck size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("read.trackers", { n: view.body.trackers })}</span>
        </p>
      )}
      <MailFrame html={html} allowRemote={view.remote_allowed} theme={theme} plain={view.body.plain || asText} />
      {(view.body.full_html || view.body.text_html) && (
        <div className="message-body-tools">
          {view.body.full_html && !asText && (
            <button type="button" className="btn btn-sm btn-quiet" aria-expanded={showQuote} onClick={() => setShowQuote(!showQuote)}>
              {showQuote ? t("read.hideQuote") : t("read.showQuote")}
            </button>
          )}
          {view.body.text_html && (
            <button type="button" className="btn btn-sm btn-quiet" aria-pressed={asText} onClick={() => setAsText(!asText)}>
              {asText ? t("read.showHtml") : t("read.showText")}
            </button>
          )}
        </div>
      )}
      {view.files.length > 0 && <Attachments files={view.files} onAttachment={p.onAttachment} />}
      <ProtectedFiles view={view} />
    </article>
  );
}

export function ReadingPane(p: Props) {
  const t = useT();
  const lang = currentLang();
  const latest = p.thread[p.thread.length - 1];
  const focus = p.thread.find((m) => m.id === p.focusId) ?? latest;
  if (!focus) {
    return (
      <section className="reading-pane reading-empty" aria-label={t("read.label")}>
        <Mail size={32} strokeWidth={1.5} aria-hidden />
        <p className="empty-title">{t("read.none")}</p>
        <p className="muted">{t("read.noneHint")}</p>
      </section>
    );
  }
  const focusView = p.views[focus.id];
  const ready = focusView && typeof focusView === "object" && "summary" in focusView ? focusView : null;
  const replyView = (() => {
    for (let i = p.thread.length - 1; i >= 0; i--) {
      const v = p.views[p.thread[i].id];
      if (v && typeof v === "object" && "summary" in v) return v;
    }
    return null;
  })();
  const tool = (a: MessageAction, icon: React.ReactNode, label: string) => (
    <button type="button" className="icon-btn" data-action={a} onClick={(e) => p.onAction(a, e.currentTarget.getBoundingClientRect())} aria-label={label} title={label}>
      {icon}
    </button>
  );
  const seen = p.thread.every((m) => m.seen);
  const flagged = p.thread.some((m) => m.flagged);
  return (
    <section className="reading-pane" aria-label={t("read.label")}>
      {p.touch ? (
        <div className="toolbar" role="toolbar" aria-label={t("read.actions")}>
          <button type="button" className="icon-btn back-btn" onClick={p.onBack} aria-label={t("common.back")}>
            <ChevronLeft size={20} strokeWidth={1.75} aria-hidden />
          </button>
          <span className="toolbar-gap" />
          {tool("snooze", <Clock size={18} strokeWidth={1.75} aria-hidden />, t("read.snooze"))}
          {tool("toggleSeen", seen ? <Mail size={18} strokeWidth={1.75} aria-hidden /> : <MailOpen size={18} strokeWidth={1.75} aria-hidden />, seen ? t("read.markUnread") : t("read.markRead"))}
          <button
            type="button"
            className="icon-btn"
            aria-pressed={flagged}
            onClick={(e) => p.onAction("flag", e.currentTarget.getBoundingClientRect())}
            aria-label={flagged ? t("read.unflag") : t("read.flag")}
            title={flagged ? t("read.unflag") : t("read.flag")}
          >
            <Flag size={18} strokeWidth={1.75} aria-hidden className={flagged ? "flag" : undefined} />
          </button>
          {tool("more", <MoreHorizontal size={18} strokeWidth={1.75} aria-hidden />, t("read.more"))}
        </div>
      ) : (
        <div className="toolbar" role="toolbar" aria-label={t("read.actions")}>
          <button type="button" className="icon-btn back-btn" onClick={p.onBack} aria-label={t("common.back")}>
            <ChevronLeft size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <button type="button" className="btn" disabled={!replyView} onClick={() => replyView && p.onCompose("reply", replyView)} title={t("read.replyKey")}>
            <Reply size={16} strokeWidth={1.75} aria-hidden />
            <span>{t("read.reply")}</span>
          </button>
          <button type="button" className="btn" disabled={!replyView} onClick={() => replyView && p.onCompose("replyAll", replyView)} title={t("read.replyAllKey")}>
            <ReplyAll size={16} strokeWidth={1.75} aria-hidden />
            <span>{t("read.replyAll")}</span>
          </button>
          <button type="button" className="btn" disabled={!replyView} onClick={() => replyView && p.onCompose("forward", replyView)} title={t("read.forwardKey")}>
            <Forward size={16} strokeWidth={1.75} aria-hidden />
            <span>{t("read.forward")}</span>
          </button>
          <span className="toolbar-gap" />
          {p.inbox
            ? tool("archive", <CheckCheck size={16} strokeWidth={1.75} aria-hidden />, t("read.doneKey"))
            : tool("archive", <Archive size={16} strokeWidth={1.75} aria-hidden />, t("read.archiveKey"))}
          {tool("snooze", <Clock size={16} strokeWidth={1.75} aria-hidden />, t("read.snoozeKey"))}
          {tool("delete", <Trash2 size={16} strokeWidth={1.75} aria-hidden />, t("read.deleteKey"))}
          {tool("spam", <ShieldAlert size={16} strokeWidth={1.75} aria-hidden />, t("read.spam"))}
          {tool("move", <FolderInput size={16} strokeWidth={1.75} aria-hidden />, t("read.move"))}
          {tool("toggleSeen", seen ? <Mail size={16} strokeWidth={1.75} aria-hidden /> : <MailOpen size={16} strokeWidth={1.75} aria-hidden />, seen ? t("read.markUnread") : t("read.markRead"))}
          <button
            type="button"
            className="icon-btn"
            aria-pressed={flagged}
            onClick={(e) => p.onAction("flag", e.currentTarget.getBoundingClientRect())}
            aria-label={flagged ? t("read.unflag") : t("read.flag")}
            title={flagged ? t("read.unflag") : t("read.flag")}
          >
            <Flag size={16} strokeWidth={1.75} aria-hidden className={flagged ? "flag" : undefined} />
          </button>
          {tool("more", <MoreHorizontal size={16} strokeWidth={1.75} aria-hidden />, t("read.more"))}
        </div>
      )}
      <div className="reading-scroll">
        {p.banner}
        <header className="message-head">
          {p.stream ? (
            <h1 className="message-subject">{t("read.stream", { n: p.thread.length, name: p.stream.name })}</h1>
          ) : (
            <h1 className="message-subject">{(ready?.summary.subject ?? focus.subject) || t("list.noSubject")}</h1>
          )}
          {p.thread.length > 1 && !p.stream && <p className="muted thread-note">{t("read.thread", { n: p.thread.length })}</p>}
          {p.thread.some((m) => m.important) && !p.stream && <span className="important-badge important-head">{t("list.important")}</span>}
        </header>
        {p.assistant}
        <div className="conversation">
          {p.thread.map((m) => {
            const v = p.views[m.id];
            const open = p.expanded.includes(m.id);
            if (!open)
              return (
                <button key={m.id} type="button" className="message-collapsed" onClick={() => p.onToggle(m.id)} aria-expanded={false}>
                  <Avatar who={m.from} size="sm" />
                  <span className={`collapsed-from${m.seen ? "" : " unread"}`}>{displayName(m.from, lang)}</span>
                  <span className="collapsed-snippet">{m.snippet}</span>
                  <span className="collapsed-date">{shortDate(m.date, now(), lang)}</span>
                </button>
              );
            if (!v || v === "loading")
              return (
                <div key={m.id} className="message-card message-loading" aria-busy="true">
                  <Spinner label={t("read.loading")} />
                  <span className="muted">{t("read.loading")}</span>
                </div>
              );
            if ("error" in v)
              return (
                <div key={m.id} className="message-card">
                  <div className="notice notice-warning" role="alert">
                    <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
                    <span>
                      <strong>{t("read.loadFailed")}</strong> {v.error}
                    </span>
                    <button type="button" className="btn btn-sm" onClick={() => p.onRetry(m.id)}>
                      {t("common.retry")}
                    </button>
                  </div>
                </div>
              );
            return (
              <MessageCard key={m.id} view={v} theme={p.theme} p={p} onCollapse={p.thread.length > 1 ? () => p.onToggle(m.id) : undefined} />
            );
          })}
        </div>
      </div>
      {p.touch && (
        <div className="touch-actions" role="toolbar" aria-label={t("read.actions")}>
          {tool("archive", p.inbox ? <CheckCheck size={20} strokeWidth={1.75} aria-hidden /> : <Archive size={20} strokeWidth={1.75} aria-hidden />, p.inbox ? t("read.done") : t("read.archive"))}
          {tool("delete", <Trash2 size={20} strokeWidth={1.75} aria-hidden />, t("read.delete"))}
          {tool("move", <FolderInput size={20} strokeWidth={1.75} aria-hidden />, t("read.move"))}
          <button type="button" className="icon-btn" disabled={!replyView} onClick={() => replyView && p.onCompose("reply", replyView)} aria-label={t("read.reply")} title={t("read.reply")}>
            <Reply size={20} strokeWidth={1.75} aria-hidden />
          </button>
          <button type="button" className="icon-btn" disabled={!replyView} onClick={() => replyView && p.onCompose("replyAll", replyView)} aria-label={t("read.replyAll")} title={t("read.replyAll")}>
            <ReplyAll size={20} strokeWidth={1.75} aria-hidden />
          </button>
          <button type="button" className="icon-btn" disabled={!replyView} onClick={() => replyView && p.onCompose("forward", replyView)} aria-label={t("read.forward")} title={t("read.forward")}>
            <Forward size={20} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
      )}
    </section>
  );
}
