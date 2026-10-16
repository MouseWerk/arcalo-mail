import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, inApp } from "./lib/api";
import { applyTheme, onSystemThemeChange, type Theme } from "./lib/theme";
import { currentLang, langFromLocale, setLang, useT } from "./lib/i18n";
import { emptyDraft, replyDraft, type ComposeMode, type ComposeState } from "./lib/compose";
import type { Account, LanguageChoice, Mailbox, MessageSummary, MessageView, Settings, ThemeChoice } from "./lib/types";
import { Sidebar, folderLabel } from "./components/Sidebar";
import { MessageList } from "./components/MessageList";
import { ReadingPane } from "./components/ReadingPane";
import { ComposeSheet } from "./components/ComposeSheet";

/** On a phone one pane is shown at a time; `view` says which (app.css, narrow layout). */
type View = "folders" | "list" | "message";

export function App() {
  const t = useT();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [theme, setTheme] = useState<Theme>("dark");
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [mailboxes, setMailboxes] = useState<Record<number, Mailbox[]>>({});
  const [mailbox, setMailbox] = useState<Mailbox | null>(null);
  const [messages, setMessages] = useState<MessageSummary[]>([]);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MessageSummary[] | null>(null);
  const [message, setMessage] = useState<MessageView | null>(null);
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [view, setView] = useState<View>("list");
  const openSeq = useRef(0);

  // Settings first: theme and language before anything else is shown.
  useEffect(() => {
    void api.settings().then(setSettings);
  }, []);

  useEffect(() => {
    if (!settings) return;
    const apply = () => setTheme(applyTheme(settings.theme));
    apply();
    setLang(settings.language === "system" ? langFromLocale(navigator.language) : settings.language);
    return onSystemThemeChange(apply);
  }, [settings]);

  const updateSettings = (patch: Partial<Settings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    void api.saveSettings(next);
  };

  // Accounts and their folders; the first inbox opens.
  useEffect(() => {
    void (async () => {
      const list = await api.accounts();
      setAccounts(list);
      const all: Record<number, Mailbox[]> = {};
      for (const a of list) all[a.id] = await api.mailboxes(a.id);
      setMailboxes(all);
      const first = list.map((a) => all[a.id]?.find((m) => m.role === "inbox")).find(Boolean);
      if (first) setMailbox(first);
    })();
  }, []);

  useEffect(() => {
    if (!mailbox) return;
    setMessage(null);
    void api.messages(mailbox.id).then(setMessages);
  }, [mailbox]);

  // Search as you type, over all accounts (local index, works offline).
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults(null);
      return;
    }
    const timer = setTimeout(() => void api.search(q, null).then(setResults), 150);
    return () => clearTimeout(timer);
  }, [query]);

  const markLocally = (id: number, seen: boolean) => {
    const patch = (list: MessageSummary[]) => list.map((m) => (m.id === id ? { ...m, seen } : m));
    setMessages(patch);
    setResults((r) => (r ? patch(r) : r));
    setMailboxes((all) => {
      const delta = seen ? -1 : 1;
      const next: Record<number, Mailbox[]> = {};
      for (const [k, list] of Object.entries(all))
        next[Number(k)] = list.map((b) => (b.id === mailbox?.id ? { ...b, unread: Math.max(0, b.unread + delta) } : b));
      return next;
    });
  };

  const open = useCallback(
    async (m: MessageSummary) => {
      const seq = ++openSeq.current;
      setView("message");
      const view = await api.message(m.id);
      if (seq !== openSeq.current) return;
      setMessage(view);
      if (!m.seen && settings?.mark_read_after_secs === 0) {
        await api.markSeen(m.id, true);
        markLocally(m.id, true);
        setMessage({ ...view, summary: { ...view.summary, seen: true } });
      }
    },
    // markLocally only uses setters and the current folder.
    [settings, mailbox],
  );

  const toggleSeen = async () => {
    if (!message) return;
    const seen = !message.summary.seen;
    await api.markSeen(message.summary.id, seen);
    markLocally(message.summary.id, seen);
    setMessage({ ...message, summary: { ...message.summary, seen } });
  };

  const allowRemote = async () => {
    if (!message) return;
    await api.allowRemote(message.summary.id);
    setMessage(await api.message(message.summary.id));
  };

  const accountOf = (m: MessageView | null): Account | null => {
    if (!m) return accounts[0] ?? null;
    const box = Object.values(mailboxes).flat().find((b) => b.id === m.summary.mailbox_id);
    return accounts.find((a) => a.id === box?.account_id) ?? accounts[0] ?? null;
  };

  const startCompose = (mode: ComposeMode) => {
    if (mode === "new" || !message) setCompose(emptyDraft((mailbox?.account_id ?? accounts[0]?.id) || null));
    else setCompose(replyDraft(message, mode, accountOf(message), currentLang()));
  };

  // Global shortcut: Ctrl/Cmd+N writes a new message.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") {
        e.preventDefault();
        startCompose("new");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const shown = results ?? messages;
  const threadCount = useMemo(() => (message ? shown.filter((m) => m.thread_id === message.summary.thread_id).length : 0), [message, shown]);
  const title = results ? t("list.searchLabel") : mailbox ? folderLabel(mailbox, t) : t("app.name");

  return (
    <div className="app" data-view={view} data-composing={compose ? "" : undefined}>
      {!inApp() && <div className="demo-bar">{t("app.demo")}</div>}
      <div className="panes">
        <Sidebar
          accounts={accounts}
          mailboxes={mailboxes}
          current={mailbox?.id ?? null}
          onSelect={(m) => {
            setQuery("");
            setMailbox(m);
            setView("list");
          }}
          onCompose={() => startCompose("new")}
          theme={settings?.theme ?? "system"}
          language={settings?.language ?? "system"}
          onTheme={(theme: ThemeChoice) => updateSettings({ theme })}
          onLanguage={(language: LanguageChoice) => updateSettings({ language })}
        />
        <MessageList
          title={title}
          messages={shown}
          current={message?.summary.id ?? null}
          query={query}
          searching={results !== null}
          onQuery={setQuery}
          onOpen={(m) => void open(m)}
          onShowFolders={() => setView("folders")}
        />
        <ReadingPane
          message={message}
          threadCount={threadCount}
          theme={theme}
          onBack={() => setView("list")}
          onCompose={startCompose}
          onToggleSeen={() => void toggleSeen()}
          onAllowRemote={() => void allowRemote()}
        />
      </div>
      {compose && <ComposeSheet draft={compose} accounts={accounts} onChange={setCompose} onClose={() => setCompose(null)} />}
    </div>
  );
}
