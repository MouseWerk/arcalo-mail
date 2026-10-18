// The AI assistant in the reading pane: a summary of the conversation (by itself for long ones
// when that is on), a reply draft in the user's tone, a translation, tasks and dates. Every
// part says where the text goes (`PrivacyLine`); an answer streams in and can be stopped.

import { AlarmClock, CalendarPlus, ChevronDown, Cloud, Languages, ListChecks, Lock, Reply, Sparkles, Square, TriangleAlert, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { openCalendar } from "../lib/calendar";
import { api } from "../lib/api";
import { destinationKey, useAi } from "../lib/ai";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { showToast } from "../lib/toasts";
import type { AiItem, AiRequest, Destination } from "../lib/types";
import { Menu, Spinner, type MenuItem } from "./ui";
import { isKey } from "../lib/ime";

/** „Bleibt auf diesem Gerät (Ollama)“ or „Geht an api.openai.com (OpenAI)“. */
export function PrivacyLine({ dest, className }: { dest: Destination | null; className?: string }) {
  const t = useT();
  const key = destinationKey(dest);
  const Icon = key === "ai.cloud" ? Cloud : Lock;
  return (
    <span className={`privacy-line${className ? ` ${className}` : ""}`} data-where={key === "ai.cloud" ? "cloud" : "local"}>
      <Icon size={13} strokeWidth={1.75} aria-hidden />
      <span>{t(key, { name: dest?.name ?? "", host: dest?.host ?? "" })}</span>
    </span>
  );
}

export const TRANSLATE_TO = ["de", "en", "fr", "es", "it", "nl", "pl", "tr", "uk"] as const;

const TITLE: Record<AiRequest["task"], TKey> = {
  summary: "ai.summary",
  reply: "ai.replyDraft",
  rewrite: "ai.rewrite",
  translate: "ai.translation",
  extract: "ai.tasks",
  priority: "ai.priority",
  ask: "ai.ask",
};

/** Text with paragraphs and „- “ points as lines. */
export function AiText({ text, busy }: { text: string; busy?: boolean }) {
  return (
    <div className="ai-text" aria-live="polite" aria-busy={busy || undefined}>
      {text.split("\n").map((l, i) => (l.trim() ? <p key={i}>{l}</p> : <br key={i} />))}
      {busy && <span className="ai-caret" aria-hidden />}
    </div>
  );
}

function itemWhen(it: AiItem, lang: "de" | "en"): string {
  if (!it.date) return "";
  const [y, m, d] = it.date.split("-").map(Number);
  const day = new Date(y, m - 1, d).toLocaleDateString(lang === "de" ? "de-DE" : "en-GB", { weekday: "short", day: "numeric", month: "long" });
  return it.time ? `${day}, ${it.time}${it.end ? `–${it.end}` : ""}` : day;
}

type Props = {
  /** The message the parts work on (the newest of the conversation for summary and reply). */
  messageId: number;
  latestId: number;
  accountId: number;
  /** The assistant is set up, and summaries of long conversations come by themselves. */
  on: boolean;
  autoSummary: boolean;
  onDraft: (text: string) => void;
  onSettings: () => void;
};

export function AssistantBar({ messageId, latestId, accountId, on, autoSummary, onDraft, onSettings }: Props) {
  const t = useT();
  const lang = currentLang();
  const ai = useAi();
  const [dest, setDest] = useState<Destination | null>(null);
  const [task, setTask] = useState<AiRequest["task"] | null>(null);
  const [notes, setNotes] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const [done, setDone] = useState<Record<number, boolean>>({});
  const asked = useRef(false);

  useEffect(() => {
    if (!on) return;
    let alive = true;
    void api.aiDestination(accountId).then((d) => {
      if (!alive) return;
      setDest(d);
      if (!d.provider || asked.current) return;
      asked.current = true;
      void api.aiThread(latestId).then((info) => {
        if (!alive) return;
        if (info.summary) {
          setTask("summary");
          void ai.run({ task: "summary", message_id: latestId });
        } else if (info.long && autoSummary) {
          setTask("summary");
          void ai.run({ task: "summary", message_id: latestId });
        }
      });
    });
    return () => {
      alive = false;
    };
  }, [on, accountId, latestId]);

  if (!on) return null;
  const allowed = !!dest?.provider;
  const start = (r: AiRequest) => {
    setTask(r.task);
    setNotes(null);
    setDone({});
    void ai.run(r);
  };
  const translateMenu = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    setMenu({
      at: { x: r.left, y: r.bottom + 4 },
      items: TRANSLATE_TO.map((code) => ({ label: t(`ai.lang.${code}` as TKey), onSelect: () => start({ task: "translate", message_id: messageId, target: code }) })),
    });
  };
  const s = ai.state;
  const showing = task && (s.busy || s.text || s.error);
  return (
    <section className="assistant" aria-label={t("ai.label")}>
      <div className="assistant-bar">
        <Sparkles size={15} strokeWidth={1.75} aria-hidden className="assistant-icon" />
        {allowed ? (
          <div className="assistant-actions" role="toolbar" aria-label={t("ai.label")}>
            <button type="button" className="btn btn-sm btn-quiet" data-ai="summary" onClick={() => start({ task: "summary", message_id: latestId })}>
              {t("ai.summarize")}
            </button>
            <button
              type="button"
              className="btn btn-sm btn-quiet"
              data-ai="reply"
              aria-expanded={notes !== null}
              onClick={() => {
                ai.reset();
                setTask(null);
                setNotes(notes === null ? "" : null);
              }}
            >
              <Reply size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("ai.draftReply")}</span>
            </button>
            <span className="split-btn split-btn-quiet">
              <button type="button" className="btn btn-sm btn-quiet" data-ai="translate" onClick={() => start({ task: "translate", message_id: messageId, target: lang })}>
                <Languages size={14} strokeWidth={1.75} aria-hidden />
                <span>{t("ai.translate")}</span>
              </button>
              <button type="button" className="btn btn-sm btn-quiet split-btn-more" aria-haspopup="menu" aria-label={t("ai.translateTo")} title={t("ai.translateTo")} onClick={(e) => translateMenu(e.currentTarget)}>
                <ChevronDown size={14} strokeWidth={1.75} aria-hidden />
              </button>
            </span>
            <button type="button" className="btn btn-sm btn-quiet" data-ai="extract" onClick={() => start({ task: "extract", message_id: messageId })}>
              <ListChecks size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("ai.findTasks")}</span>
            </button>
          </div>
        ) : (
          <span className="assistant-off muted">
            {t("ai.notAllowed")}{" "}
            <button type="button" className="link-btn" onClick={onSettings}>
              {t("ai.openSettings")}
            </button>
          </span>
        )}
        {allowed && <PrivacyLine dest={dest} />}
      </div>
      {notes !== null && (
        <form
          className="assistant-notes"
          onSubmit={(e) => {
            e.preventDefault();
            start({ task: "reply", message_id: latestId, instructions: notes });
          }}
        >
          <input
            className="input"
            autoFocus
            value={notes}
            placeholder={t("ai.notesPlaceholder")}
            aria-label={t("ai.notes")}
            onChange={(e) => setNotes(e.target.value)}
            onKeyDown={(e) => isKey(e, "Escape") && (e.stopPropagation(), setNotes(null))}
          />
          <button type="submit" className="btn btn-sm">
            {t("ai.draft")}
          </button>
        </form>
      )}
      {showing && (
        <div className="assistant-out" data-task={task}>
          <header className="assistant-out-head">
            <span className="assistant-out-title">{t(TITLE[task])}</span>
            {s.outcome?.cached && <span className="muted assistant-kept">{t("ai.keptBadge")}</span>}
            <span className="toolbar-gap" />
            {s.busy ? (
              <button type="button" className="btn btn-sm btn-quiet" onClick={ai.cancel}>
                <Square size={12} strokeWidth={2} aria-hidden />
                <span>{t("ai.stop")}</span>
              </button>
            ) : (
              <button
                type="button"
                className="icon-btn icon-btn-sm"
                aria-label={t("common.close")}
                onClick={() => {
                  ai.reset();
                  setTask(null);
                }}
              >
                <X size={14} strokeWidth={1.75} aria-hidden />
              </button>
            )}
          </header>
          {s.busy && !s.text && (
            <p className="muted assistant-wait">
              <Spinner /> {dest?.local ? t("ai.thinkingLocal") : t("ai.thinking")}
            </p>
          )}
          {s.text && task !== "extract" && <AiText text={s.text} busy={s.busy} />}
          {s.outcome?.cancelled && <p className="muted">{t("ai.stopped")}</p>}
          {s.error && (
            <div className="notice notice-warning" role="alert">
              <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
              <span>{s.error}</span>
              <span className="notice-actions">
                <button type="button" className="btn btn-sm" onClick={() => task === "summary" ? start({ task: "summary", message_id: latestId }) : task && start(task === "translate" ? { task, message_id: messageId, target: lang } : task === "extract" ? { task, message_id: messageId } : { task: "reply", message_id: latestId, instructions: "" })}>
                  {t("common.retry")}
                </button>
              </span>
            </div>
          )}
          {task === "extract" && s.busy && s.text && <p className="muted assistant-wait"><Spinner /> {t("ai.reading")}</p>}
          {task === "extract" && s.outcome && !s.outcome.cancelled && (
            s.outcome.items.length === 0 ? (
              <p className="muted">{t("ai.noTasks")}</p>
            ) : (
              <ul className="ai-items">
                {s.outcome.items.map((it, i) => (
                  <li key={i} className="ai-item" data-kind={it.kind}>
                    <span className="ai-item-icon">{it.kind === "event" ? <CalendarPlus size={15} strokeWidth={1.75} aria-hidden /> : <ListChecks size={15} strokeWidth={1.75} aria-hidden />}</span>
                    <span className="ai-item-text">
                      <span className="ai-item-title">{it.title}</span>
                      <span className="muted ai-item-when">{[itemWhen(it, lang), it.place].filter(Boolean).join(" · ") || t("ai.noDate")}</span>
                    </span>
                    <span className="ai-item-actions">
                      {it.at != null && (
                        <button
                          type="button"
                          className="btn btn-sm btn-quiet"
                          disabled={done[i]}
                          onClick={() =>
                            void api
                              .aiRemind(messageId, it.at ?? 0, it.title)
                              .then(() => {
                                setDone((d) => ({ ...d, [i]: true }));
                                showToast({ title: t("ai.reminderSet", { when: itemWhen(it, lang) }), tone: "success", duration: 3000 });
                              })
                              .catch((e) => showToast({ title: String(e), tone: "danger" }))
                          }
                        >
                          <AlarmClock size={14} strokeWidth={1.75} aria-hidden />
                          <span>{done[i] ? t("ai.reminderDone") : t("ai.remind")}</span>
                        </button>
                      )}
                      {it.date && (
                        <button
                          type="button"
                          className="btn btn-sm btn-quiet"
                          onClick={() =>
                            void api
                              .aiCalendar(messageId, it)
                              .then((draft) => openCalendar({ draft }))
                              .catch((e) => showToast({ title: String(e), tone: "danger" }))
                          }
                        >
                          <CalendarPlus size={14} strokeWidth={1.75} aria-hidden />
                          <span>{t("ai.toCalendar")}</span>
                        </button>
                      )}
                    </span>
                  </li>
                ))}
              </ul>
            )
          )}
          {task === "reply" && !s.busy && s.text && !s.error && (
            <div className="assistant-out-foot">
              <button type="button" className="btn btn-sm btn-primary" onClick={() => onDraft(s.text)}>
                <Reply size={14} strokeWidth={1.75} aria-hidden />
                <span>{t("ai.useDraft")}</span>
              </button>
              <button type="button" className="btn btn-sm btn-quiet" onClick={() => setNotes("")}>
                {t("ai.again")}
              </button>
            </div>
          )}
        </div>
      )}
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("ai.translateTo")} />}
    </section>
  );
}
