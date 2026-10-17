// „Frag dein Postfach“: a question in plain words; the local search finds the messages, the
// account's provider answers from them, and every `[n]` links to its message.

import { ArrowUp, Menu as MenuIcon, MessagesSquare, Square, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { citeParts, useAi } from "../lib/ai";
import { currentLang, useT } from "../lib/i18n";
import { listDate, now } from "../lib/format";
import type { AiSource, Destination } from "../lib/types";
import { PrivacyLine } from "./Assistant";
import { Spinner } from "./ui";

export function AskView({ on, onOpen, onShowFolders, onSettings }: { on: boolean; onOpen: (s: AiSource) => void; onShowFolders: () => void; onSettings: () => void }) {
  const t = useT();
  const lang = currentLang();
  const ai = useAi();
  const [question, setQuestion] = useState("");
  const [asked, setAsked] = useState("");
  const [providers, setProviders] = useState<Destination[]>([]);
  const [provider, setProvider] = useState<string | null>(null);
  useEffect(() => {
    if (!on) return;
    void api.aiAskProviders().then((list) => {
      setProviders(list);
      setProvider((p) => p ?? list[0]?.provider ?? null);
    });
  }, [on]);
  const dest = providers.find((p) => p.provider === provider) ?? providers[0] ?? null;
  const ask = (q: string) => {
    if (!q.trim()) return;
    setAsked(q.trim());
    void ai.run({ task: "ask", question: q.trim(), provider });
  };
  const s = ai.state;
  const sources = s.outcome?.sources ?? [];
  const byN = new Map(sources.map((x) => [x.n, x]));
  return (
    <section className="list-pane smart-pane ask-pane" aria-label={t("ask.title")}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{t("ask.title")}</h1>
        </div>
        <p className="muted smart-note">{t("ask.note")}</p>
      </header>
      {!on || providers.length === 0 ? (
        <div className="empty smart-empty">
          <MessagesSquare size={28} strokeWidth={1.5} aria-hidden />
          <p>{t("ask.off")}</p>
          <button type="button" className="btn btn-sm" onClick={onSettings}>
            {t("ai.openSettings")}
          </button>
        </div>
      ) : (
        <div className="ask-body">
          <form
            className="ask-form"
            onSubmit={(e) => {
              e.preventDefault();
              ask(question);
            }}
          >
            <textarea
              className="input ask-input"
              rows={2}
              value={question}
              placeholder={t("ask.placeholder")}
              aria-label={t("ask.question")}
              onChange={(e) => setQuestion(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  ask(question);
                }
              }}
            />
            {s.busy ? (
              <button type="button" className="icon-btn ask-send" onClick={ai.cancel} aria-label={t("ai.stop")} title={t("ai.stop")}>
                <Square size={14} strokeWidth={2} aria-hidden />
              </button>
            ) : (
              <button type="submit" className="icon-btn ask-send" disabled={!question.trim()} aria-label={t("ask.send")} title={t("ask.send")}>
                <ArrowUp size={16} strokeWidth={2} aria-hidden />
              </button>
            )}
          </form>
          <div className="ask-meta">
            {providers.length > 1 && (
              <select className="select select-sm" value={provider ?? ""} onChange={(e) => setProvider(e.target.value)} aria-label={t("ask.provider")}>
                {providers.map((p) => (
                  <option key={p.provider} value={p.provider ?? ""}>
                    {p.name}
                  </option>
                ))}
              </select>
            )}
            <PrivacyLine dest={dest} />
          </div>
          {!asked && (
            <div className="ask-examples">
              {(["ask.example1", "ask.example2", "ask.example3"] as const).map((k) => (
                <button
                  key={k}
                  type="button"
                  className="chip chip-btn"
                  onClick={() => {
                    setQuestion(t(k));
                    ask(t(k));
                  }}
                >
                  {t(k)}
                </button>
              ))}
            </div>
          )}
          {asked && (
            <article className="ask-answer" aria-label={t("ask.answer")}>
              <p className="ask-question">{asked}</p>
              {s.busy && !s.text && (
                <p className="muted assistant-wait">
                  <Spinner /> {t("ask.searching")}
                </p>
              )}
              {s.text && (
                <div className="ai-text" aria-live="polite">
                  {s.text.split("\n").map((line, i) =>
                    line.trim() ? (
                      <p key={i}>
                        {citeParts(line).map((part, j) =>
                          "ref" in part ? (
                            byN.get(part.ref) ? (
                              <button key={j} type="button" className="cite" onClick={() => onOpen(byN.get(part.ref)!)} title={byN.get(part.ref)!.subject}>
                                {part.ref}
                              </button>
                            ) : (
                              <span key={j} className="cite cite-plain">
                                {part.ref}
                              </span>
                            )
                          ) : (
                            <span key={j}>{part.text}</span>
                          ),
                        )}
                      </p>
                    ) : (
                      <br key={i} />
                    ),
                  )}
                </div>
              )}
              {s.error && (
                <div className="notice notice-warning" role="alert">
                  <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
                  <span>{s.error}</span>
                </div>
              )}
              {sources.length > 0 && (
                <>
                  <h2 className="ask-sources-title">{t("ask.sources")}</h2>
                  <ul className="smart-list ask-sources">
                    {sources.map((src) => (
                      <li key={src.n} className="smart-item">
                        <button type="button" className="smart-main" onClick={() => onOpen(src)}>
                          <span className="cite cite-plain" aria-hidden>
                            {src.n}
                          </span>
                          <span className="smart-text">
                            <span className="smart-title">
                              <span className="smart-name">{src.subject || t("list.noSubject")}</span>
                            </span>
                            <span className="smart-meta">
                              <span className="smart-subject">{src.from}</span>
                              <span className="smart-date">{listDate(src.date, now(), lang)}</span>
                            </span>
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </article>
          )}
        </div>
      )}
    </section>
  );
}
