import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { api, inApp, onNotice, pickSavePath } from "./lib/api";
import { applyTheme, onSystemThemeChange, type Theme } from "./lib/theme";
import { currentLang, langFromLocale, setLang, useT } from "./lib/i18n";
import { draftFromMessage, emptyDraft, isEmpty, isValidAddress, replyDraft, toDraft, type ComposeState } from "./lib/compose";
import { clickSelect, isTyping, keyAction, pagesFor, type Selection } from "./lib/listing";
import { closeToast, runUndo, setUndo, showToast } from "./lib/toasts";
import { setClock } from "./lib/format";
import type { AccountInfo, AppInfo, Attachment, Mailbox, MessageSummary, MessageView, Notice, OutboxEntry, Settings, Undo } from "./lib/types";
import { Sidebar, folderIcon, folderLabel, type FolderAction } from "./components/Sidebar";
import { MessageList, type BulkAction } from "./components/MessageList";
import { ReadingPane, type Loaded, type MessageAction } from "./components/ReadingPane";
import { ComposeSheet } from "./components/ComposeSheet";
import { MAILTO_EVENT } from "./components/MailFrame";
import { Setup } from "./components/Setup";
import { SettingsView, type SettingsSection } from "./components/Settings";
import { Toasts } from "./components/Toasts";
import { Outbox } from "./components/Outbox";
import { ConfirmDialog, Dialog, Menu, PromptDialog, type MenuItem } from "./components/ui";

/** On a phone one pane is shown at a time; `view` says which (app.css, narrow layout). */
type View = "folders" | "list" | "message";

const PAGE = 100;

type ListState = { key: string; total: number; pages: Record<number, MessageSummary[]>; loading: boolean };

type FolderDialog = FolderAction | null;

export function App() {
  const t = useT();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [theme, setTheme] = useState<Theme>("dark");
  const [accounts, setAccounts] = useState<AccountInfo[] | null>(null);
  const [mailboxes, setMailboxes] = useState<Record<number, Mailbox[]>>({});
  const [mailboxId, setMailboxId] = useState<number | null>(null);
  const [view, setView] = useState<View>("list");
  const [list, setList] = useState<ListState>({ key: "", total: 0, pages: {}, loading: true });
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MessageSummary[] | null>(null);
  // Bumped when the store changed (new mail, fetched bodies): open search results follow.
  const [searchRev, setSearchRev] = useState(0);
  const serverHits = useRef(new Set<number>());
  const [server, setServer] = useState({ busy: false, done: false });
  const [sel, setSel] = useState<Selection>({ ids: [], anchor: null });
  const [current, setCurrent] = useState<number | null>(null);
  const [thread, setThread] = useState<MessageSummary[]>([]);
  const [views, setViews] = useState<Record<number, Loaded>>({});
  const [expanded, setExpanded] = useState<number[]>([]);
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [composeError, setComposeError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState<SettingsSection | null>(null);
  const [showOutbox, setShowOutbox] = useState(false);
  const [outbox, setOutbox] = useState<OutboxEntry[]>([]);
  const [folderDialog, setFolderDialog] = useState<FolderDialog>(null);
  const [signIn, setSignIn] = useState<AccountInfo | null>(null);
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const [preview, setPreview] = useState<{ name: string; url: string } | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const openSeq = useRef(0);
  const lastSync = useRef(0);

  const mailbox = useMemo(() => Object.values(mailboxes).flat().find((m) => m.id === mailboxId) ?? null, [mailboxes, mailboxId]);
  const threads = settings?.threads ?? true;
  const listKey = `${mailboxId}:${threads}`;

  // ---------------------------------------------------------------- start

  useEffect(() => {
    void Promise.all([api.appInfo(), api.settings()]).then(([i, s]) => {
      setClock(i.clock);
      setInfo(i);
      setSettings(s);
    });
  }, []);

  useEffect(() => {
    if (!settings || !info) return;
    const apply = () => setTheme(applyTheme(settings.theme));
    apply();
    setLang(settings.language === "system" ? langFromLocale(info.locale || navigator.language) : settings.language);
    return onSystemThemeChange(apply);
  }, [settings, info]);

  const loadAccounts = useCallback(async () => {
    const list = await api.accounts();
    setAccounts(list);
    const all: Record<number, Mailbox[]> = {};
    for (const a of list) all[a.id] = await api.mailboxes(a.id);
    setMailboxes(all);
    return { list, all };
  }, []);

  useEffect(() => {
    void loadAccounts().then(({ list, all }) => {
      const first = list.map((a) => all[a.id]?.find((m) => m.role === "inbox")).find(Boolean);
      if (first) setMailboxId(first.id);
    });
    void api.outbox().then(setOutbox);
  }, []);

  useEffect(() => {
    if (settings && accounts && info) document.body.classList.add("ready");
  }, [settings, accounts, info]);

  const reloadMailboxes = useCallback(async (accountId: number) => {
    const boxes = await api.mailboxes(accountId);
    setMailboxes((all) => ({ ...all, [accountId]: boxes }));
    // The first sync brings the folders: open the inbox once it is there.
    setMailboxId((cur) => cur ?? boxes.find((b) => b.role === "inbox")?.id ?? null);
  }, []);

  // ---------------------------------------------------------------- the list

  const fetchPage = useCallback(async (key: string, mbox: number, page: number, thr: boolean) => {
    const r = await api.messages(mbox, page * PAGE, PAGE, thr);
    setList((l) => (l.key === key ? { ...l, total: r.total, pages: { ...l.pages, [page]: r.items }, loading: false } : l));
  }, []);

  const refreshList = useCallback(() => {
    if (mailboxId == null) return;
    setList((l) => {
      const pages = Object.keys(l.pages).map(Number);
      for (const p of pages.length ? pages : [0]) void fetchPage(l.key, mailboxId, p, threads);
      return l;
    });
  }, [mailboxId, threads, fetchPage]);

  useEffect(() => {
    if (mailboxId == null) return;
    setList({ key: listKey, total: 0, pages: {}, loading: true });
    setSel({ ids: [], anchor: null });
    setCurrent(null);
    setThread([]);
    void fetchPage(listKey, mailboxId, 0, threads);
  }, [listKey]);

  const onRange = useCallback(
    (start: number, end: number) => {
      if (results || mailboxId == null) return;
      for (const p of pagesFor(start, end, PAGE)) if (!list.pages[p]) void fetchPage(list.key, mailboxId, p, threads);
    },
    [results, mailboxId, list.pages, list.key, threads, fetchPage],
  );

  const rowAt = useCallback(
    (i: number): MessageSummary | undefined => (results ? results[i] : list.pages[Math.floor(i / PAGE)]?.[i % PAGE]),
    [results, list.pages],
  );
  const total = results ? results.length : list.total;
  const order = useMemo(() => Array.from({ length: total }, (_, i) => rowAt(i)?.id), [total, rowAt]);

  // Search as you type, over all accounts (local index, works offline).
  useEffect(() => {
    const q = query.trim();
    setServer({ busy: false, done: false });
    serverHits.current = new Set();
    if (!q) {
      setResults(null);
      return;
    }
    const timer = setTimeout(() => void api.search(q, null).then(setResults), 120);
    return () => clearTimeout(timer);
  }, [query]);

  // The store changed under open results: search again, keeping what the server found.
  useEffect(() => {
    const q = query.trim();
    if (!q || searchRev === 0) return;
    const timer = setTimeout(
      () =>
        void api.search(q, null).then((local) =>
          setResults((r) => {
            if (!r) return r;
            const known = new Set(local.map((m) => m.id));
            return [...local, ...r.filter((m) => serverHits.current.has(m.id) && !known.has(m.id))];
          }),
        ),
      300,
    );
    return () => clearTimeout(timer);
  }, [searchRev]);

  const serverSearch = async () => {
    const accountId = mailbox?.account_id ?? accounts?.[0]?.id;
    if (accountId == null || !query.trim()) return;
    setServer({ busy: true, done: false });
    try {
      const more = await api.serverSearch(accountId, query.trim());
      for (const m of more) serverHits.current.add(m.id);
      setResults((r) => {
        const known = new Set((r ?? []).map((m) => m.id));
        return [...(r ?? []), ...more.filter((m) => !known.has(m.id))];
      });
      setServer({ busy: false, done: true });
    } catch (e) {
      setServer({ busy: false, done: false });
      showToast({ title: t("list.serverSearchFailed"), detail: String(e), tone: "danger" });
    }
  };

  // ---------------------------------------------------------------- reading

  const loadView = useCallback(async (id: number) => {
    setViews((v) => ({ ...v, [id]: v[id] && v[id] !== "loading" ? v[id] : "loading" }));
    try {
      const m = await api.message(id);
      setViews((v) => ({ ...v, [id]: m }));
      return m;
    } catch (e) {
      setViews((v) => ({ ...v, [id]: { error: String(e) } }));
      return null;
    }
  }, []);

  const patchRows = useCallback((ids: number[], patch: Partial<MessageSummary>) => {
    const f = (m: MessageSummary) => (ids.includes(m.id) ? { ...m, ...patch } : m);
    setList((l) => ({ ...l, pages: Object.fromEntries(Object.entries(l.pages).map(([k, v]) => [k, v.map(f)])) }));
    setResults((r) => (r ? r.map(f) : r));
    setThread((th) => th.map(f));
  }, []);

  const open = useCallback(
    async (m: MessageSummary) => {
      const seq = ++openSeq.current;
      setCurrent(m.id);
      setView("message");
      const list = threads && m.thread_count > 1 ? await api.thread(m.thread_id) : [m];
      if (seq !== openSeq.current) return;
      const th = list.length ? list : [m];
      setThread(th);
      const open = [...new Set([...th.filter((x) => !x.seen).map((x) => x.id), th[th.length - 1].id])];
      setExpanded(open);
      setViews({});
      await Promise.all(open.map((id) => loadView(id)));
      if (seq !== openSeq.current) return;
      const unread = th.filter((x) => !x.seen).map((x) => x.id);
      const after = settings?.mark_read_after_secs;
      if (unread.length && after != null) {
        const mark = () => {
          if (seq !== openSeq.current) return;
          void api.setFlags(unread, true, null);
          patchRows(unread, { seen: true });
          if (m.thread_count > 1) patchRows([m.id], { seen: true });
          setMailboxes((all) => {
            const next: typeof all = {};
            for (const [k, boxes] of Object.entries(all))
              next[Number(k)] = boxes.map((b) => {
                const n = th.filter((x) => unread.includes(x.id) && x.mailbox_id === b.id).length;
                return n ? { ...b, unread: Math.max(0, b.unread - n) } : b;
              });
            return next;
          });
        };
        if (after === 0) mark();
        else setTimeout(mark, after * 1000);
      }
    },
    [threads, settings, loadView, patchRows],
  );

  const toggleExpanded = (id: number) => {
    setExpanded((e) => (e.includes(id) ? e.filter((x) => x !== id) : [...e, id]));
    if (!views[id]) void loadView(id);
  };

  // ---------------------------------------------------------------- actions

  /** What an action applies to: the selection, else the open conversation in this folder. */
  const targets = (): number[] => {
    if (sel.ids.length > 1) return sel.ids;
    if (!thread.length) return current != null ? [current] : [];
    const here = thread.filter((m) => results || m.mailbox_id === mailboxId).map((m) => m.id);
    return here.length ? here : current != null ? [current] : [];
  };

  /** After rows went away: the next one opens (the one below, else above). */
  const afterRemoval = (gone: number[]) => {
    const idx = order.findIndex((id) => id !== undefined && gone.includes(id));
    const next = order.slice(idx + 1).find((id) => id !== undefined && !gone.includes(id)) ?? order.slice(0, Math.max(0, idx)).reverse().find((id) => id !== undefined && !gone.includes(id));
    setSel({ ids: [], anchor: null });
    setResults((r) => (r ? r.filter((m) => !gone.includes(m.id)) : r));
    const nextRow = next !== undefined ? order.indexOf(next) : -1;
    const row = nextRow >= 0 ? rowAt(nextRow) : undefined;
    refreshList();
    if (row && current != null && gone.includes(current)) void open(row);
    else if (current != null && gone.includes(current)) {
      setCurrent(null);
      setThread([]);
    }
  };

  const reloadCounts = () => {
    for (const a of accounts ?? []) void reloadMailboxes(a.id);
  };

  const withUndo = async (label: string, ids: number[], run: () => Promise<Undo>) => {
    if (!ids.length) return;
    try {
      const token = await run();
      afterRemoval(ids);
      reloadCounts();
      const undo = async () => {
        const ok = await api.undo(token);
        refreshList();
        reloadCounts();
        showToast({ title: ok ? t("toast.undone") : t("toast.tooLate"), tone: ok ? "info" : "danger", duration: 3000 });
      };
      setUndo(() => void undo());
      showToast({ title: label, action: { label: t("common.undo"), run: () => void undo() }, duration: 5000 });
    } catch (e) {
      showToast({ title: t("toast.actionFailed"), detail: String(e), tone: "danger" });
    }
  };

  const setFlags = async (ids: number[], seen: boolean | null, flagged: boolean | null) => {
    if (!ids.length) return;
    await api.setFlags(ids, seen, flagged);
    const patch: Partial<MessageSummary> = {};
    if (seen != null) patch.seen = seen;
    if (flagged != null) patch.flagged = flagged;
    patchRows(ids, patch);
    reloadCounts();
    if (threads) refreshList();
  };

  const moveMenu = (ids: number[], at: { x: number; y: number }) => {
    const first = [...thread, ...(results ?? []), ...Object.values(list.pages).flat()].find((m) => ids.includes(m.id));
    const box = Object.values(mailboxes).flat().find((b) => b.id === first?.mailbox_id);
    if (!box) return;
    const items: MenuItem[] = (mailboxes[box.account_id] ?? [])
      .filter((b) => b.selectable && b.id !== box.id && b.role !== "drafts")
      .map((b) => {
        const Icon = folderIcon(b);
        return {
          label: folderLabel(b, t),
          icon: <Icon size={14} aria-hidden />,
          onSelect: () => void withUndo(t("toast.moved", { folder: folderLabel(b, t) }), ids, () => api.move(ids, b.id)),
        };
      });
    setMenu({ items, at });
  };

  const act = (a: MessageAction | BulkAction, anchor?: DOMRect) => {
    const ids = targets();
    if (!ids.length) return;
    const rows = [...thread, ...(results ?? []), ...Object.values(list.pages).flat()].filter((m) => ids.includes(m.id));
    switch (a) {
      case "archive":
        void withUndo(t("toast.archived", { n: ids.length }), ids, () => api.archive(ids));
        break;
      case "delete":
        void withUndo(t("toast.deleted", { n: ids.length }), ids, () => api.remove(ids));
        break;
      case "spam":
        void withUndo(t("toast.spam", { n: ids.length }), ids, () => api.spam(ids));
        break;
      case "move":
        moveMenu(ids, anchor ? { x: anchor.left, y: anchor.bottom + 4 } : { x: 300, y: 120 });
        break;
      case "toggleSeen":
        void setFlags(ids, !rows.every((m) => m.seen), null);
        break;
      case "read":
        void setFlags(ids, true, null);
        break;
      case "unread":
        void setFlags(ids, false, null);
        break;
      case "flag":
        void setFlags(ids, null, !rows.some((m) => m.flagged));
        break;
    }
  };

  // ---------------------------------------------------------------- compose

  const accountById = (id: number | null | undefined) => accounts?.find((a) => a.id === id) ?? null;
  const rich = settings?.compose_rich_text ?? true;

  const saveDraft = async (d: ComposeState, quiet: boolean) => {
    if (d.accountId == null) return d;
    try {
      const messageId = await api.saveDraft(toDraft(d));
      if (!quiet) showToast({ title: t("toast.draftSaved"), duration: 2500 });
      return { ...d, messageId };
    } catch (e) {
      showToast({ title: t("toast.draftFailed"), detail: String(e), tone: "danger" });
      return d;
    }
  };

  const startCompose = async (next: ComposeState) => {
    if (compose && !isEmpty(compose)) await saveDraft(compose, false);
    setComposeError(null);
    setCompose(next);
  };

  const newMessage = (to: string[] = []) => {
    const account = accountById(mailbox?.account_id) ?? accounts?.[0] ?? null;
    void startCompose(emptyDraft(account, rich, to));
  };

  const replyTo = (mode: "reply" | "replyAll" | "forward", v: MessageView) => {
    void startCompose(replyDraft(v, mode, accountById(v.account_id), currentLang(), rich));
  };

  const latestView = (): MessageView | null => {
    for (let i = thread.length - 1; i >= 0; i--) {
      const v = views[thread[i].id];
      if (v && typeof v === "object" && "summary" in v) return v;
    }
    return null;
  };

  const send = async () => {
    if (!compose || sending) return;
    const all = [...compose.to, ...compose.cc, ...compose.bcc];
    if (!all.length) {
      setComposeError(t("compose.noRecipient"));
      return;
    }
    const bad = all.find((a) => !isValidAddress(a));
    if (bad) {
      setComposeError(t("compose.invalidNamed", { address: bad }));
      return;
    }
    setSending(true);
    setComposeError(null);
    const state = compose;
    try {
      const q = await api.send(toDraft(state));
      setCompose(null);
      const toastId = showToast({
        title: q.delay > 0 ? t("toast.sending") : t("toast.queued"),
        duration: q.delay > 0 ? q.delay * 1000 : 3000,
        action:
          q.delay > 0
            ? {
                label: t("common.undo"),
                run: () =>
                  void api.cancelSend(q.outbox_id).then((ok) => {
                    if (ok) {
                      setCompose({ ...state, messageId: state.messageId, revision: state.revision + 1 });
                      showToast({ title: t("toast.sendCancelled"), duration: 3000 });
                    } else showToast({ title: t("toast.alreadySent"), tone: "danger", duration: 3000 });
                  }),
              }
            : undefined,
      });
      setUndo(() => {
        closeToast(toastId);
        void api.cancelSend(q.outbox_id).then((ok) => ok && setCompose(state));
      }, q.delay * 1000);
      void api.outbox().then(setOutbox);
    } catch (e) {
      setComposeError(String(e));
    } finally {
      setSending(false);
    }
  };

  const closeCompose = async () => {
    if (!compose) return;
    const d = compose;
    setCompose(null);
    if (!isEmpty(d)) await saveDraft(d, false);
  };

  const discardCompose = async () => {
    if (!compose) return;
    const d = compose;
    setCompose(null);
    if (d.messageId && d.accountId != null) await api.discardDraft(d.accountId, d.messageId);
    showToast({ title: t("toast.draftDiscarded"), duration: 2500 });
  };

  useEffect(() => {
    const onMailto = (e: Event) => {
      const href = String((e as CustomEvent).detail);
      const to = decodeURIComponent(href.replace(/^mailto:/i, "").split("?")[0]);
      newMessage(to ? [to] : []);
    };
    window.addEventListener(MAILTO_EVENT, onMailto);
    return () => window.removeEventListener(MAILTO_EVENT, onMailto);
  });

  // ---------------------------------------------------------------- attachments

  const attachment = async (a: Attachment, how: "open" | "save" | "preview") => {
    const name = a.filename ?? t("read.unnamed");
    try {
      if (how === "preview") {
        const p = await api.previewAttachment(a.id);
        setPreview({ name, url: `data:${p.mime_type};base64,${p.data_b64}` });
      } else if (how === "open") {
        await api.openAttachment(a.id);
      } else {
        const path = await pickSavePath(name);
        if (!path) return;
        await api.saveAttachment(a.id, path);
        showToast({ title: t("toast.saved", { name }), duration: 3000, tone: "success" });
      }
    } catch (e) {
      showToast({ title: t("read.attachmentFailed"), detail: String(e), tone: "danger" });
    }
  };

  // ---------------------------------------------------------------- notices

  useEffect(
    () =>
      onNotice((n: Notice) => {
        switch (n.kind) {
          case "status":
            setAccounts((list) => list?.map((a) => (a.id === n.account_id ? { ...a, status: n } : a)) ?? list);
            break;
          case "changed":
            void reloadMailboxes(n.account_id);
            setSearchRev((v) => v + 1);
            if (!n.mailboxes.length || (mailboxId != null && n.mailboxes.includes(mailboxId))) refreshList();
            break;
          case "new_mail":
            void reloadMailboxes(n.account_id);
            break;
          case "outbox":
            void api.outbox().then(setOutbox);
            void api.accounts().then((list) => setAccounts((cur) => list.map((a) => ({ ...a, status: cur?.find((c) => c.id === a.id)?.status ?? a.status }))));
            break;
          case "sent":
            showToast({ title: t("toast.sent"), tone: "success", duration: 3000 });
            break;
        }
      }),
    [mailboxId, refreshList, reloadMailboxes],
  );

  // Back in front after a while: check for mail now.
  useEffect(() => {
    const onFocus = () => {
      if (Date.now() - lastSync.current > 60_000) {
        lastSync.current = Date.now();
        void api.syncNow(null);
      }
    };
    lastSync.current = Date.now();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // ---------------------------------------------------------------- settings

  const updateSettings = (patch: Partial<Settings>) => {
    if (!settings) return;
    const before = settings;
    const next = { ...settings, ...patch };
    setSettings(next);
    void api.saveSettings(next);
    showToast({
      title: t("toast.settingApplied"),
      duration: 4000,
      action: {
        label: t("common.undo"),
        run: () => {
          setSettings(before);
          void api.saveSettings(before);
        },
      },
    });
  };

  // ---------------------------------------------------------------- keyboard

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const overlay = document.querySelector(".overlay, .menu");
      const inCompose = (e.target as HTMLElement | null)?.closest?.(".compose");
      const action = keyAction(e, isTyping(e.target) || !!inCompose);
      if (!action || (overlay && action !== "settings")) return;
      const index = current != null ? order.indexOf(current) : -1;
      const go = (i: number) => {
        const row = rowAt(i);
        if (!row) return;
        setSel({ ids: [row.id], anchor: row.id });
        void open(row);
      };
      switch (action) {
        case "next":
          e.preventDefault();
          go(index < 0 ? 0 : Math.min(total - 1, index + 1));
          break;
        case "prev":
          e.preventDefault();
          go(index < 0 ? 0 : Math.max(0, index - 1));
          break;
        case "open":
          if (index >= 0) go(index);
          else go(0);
          break;
        case "archive":
        case "delete":
        case "spam":
        case "flag":
          e.preventDefault();
          act(action);
          break;
        case "markRead":
          act("read");
          break;
        case "markUnread":
          act("unread");
          break;
        case "reply":
        case "replyAll":
        case "forward": {
          const v = latestView();
          if (v) {
            e.preventDefault();
            replyTo(action, v);
          }
          break;
        }
        case "compose":
          e.preventDefault();
          newMessage();
          break;
        case "search":
          e.preventDefault();
          searchRef.current?.focus();
          searchRef.current?.select();
          break;
        case "undo":
          if (!runUndo()) showToast({ title: t("toast.nothingToUndo"), duration: 2000 });
          break;
        case "select":
          if (current != null) setSel((s) => ({ ids: s.ids.includes(current) ? s.ids.filter((x) => x !== current) : [...s.ids, current], anchor: current }));
          break;
        case "escape":
          if (sel.ids.length > 1) setSel({ ids: current != null ? [current] : [], anchor: current });
          else if (view === "message") setView("list");
          break;
        case "settings":
          e.preventDefault();
          setSettingsOpen((s) => (s ? null : "accounts"));
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // ---------------------------------------------------------------- render

  if (!settings || !accounts || !info) return <div className="app app-loading" />;

  if (accounts.length === 0)
    return (
      <div className="app app-setup">
        {!inApp() && <div className="demo-bar">{t("app.demo")}</div>}
        <Setup
          welcome
          onDone={() =>
            void loadAccounts().then(({ list, all }) => {
              const first = list.map((a) => all[a.id]?.find((m) => m.role === "inbox")).find(Boolean);
              setMailboxId(first?.id ?? null);
            })
          }
        />
        <Toasts />
      </div>
    );

  const title = results ? t("list.searchLabel") : mailbox ? folderLabel(mailbox, t) : t("app.name");
  const emptyText = results ? t("list.noResults", { q: query.trim() }) : mailbox ? t("list.empty") : t("list.syncing");

  return (
    <div className="app" data-view={view} data-composing={compose ? "" : undefined}>
      {!inApp() && <div className="demo-bar">{t("app.demo")}</div>}
      <div className="panes">
        <Sidebar
          accounts={accounts}
          mailboxes={mailboxes}
          current={mailboxId}
          showingOutbox={showOutbox}
          outboxCount={outbox.length}
          onSelect={(m) => {
            setQuery("");
            setShowOutbox(false);
            setMailboxId(m.id);
            setView("list");
          }}
          onOutbox={() => {
            setShowOutbox(true);
            setView("list");
            void api.outbox().then(setOutbox);
          }}
          onCompose={() => newMessage()}
          onSettings={() => setSettingsOpen("accounts")}
          onFolder={(a) => setFolderDialog(a)}
          onSignIn={(a) => setSignIn(a)}
          onSync={(a) => void api.syncNow(a.id)}
        />
        {showOutbox ? (
          <Outbox
            entries={outbox}
            accounts={accounts}
            onShowFolders={() => setView("folders")}
            onRetry={(id) => void api.retryOutbox(id).then(() => api.outbox().then(setOutbox))}
            onDiscard={(id) =>
              void api.discardOutbox(id).then(() => {
                void api.outbox().then(setOutbox);
                refreshList();
                reloadCounts();
              })
            }
          />
        ) : (
          <MessageList
            title={title}
            total={total}
            rowAt={rowAt}
            onRange={onRange}
            selected={sel.ids}
            current={current}
            query={query}
            searching={results !== null}
            serverSearch={results !== null ? server : null}
            showSnippets={settings.show_snippets}
            loading={!results && list.loading}
            emptyText={emptyText}
            onQuery={setQuery}
            onServerSearch={() => void serverSearch()}
            onRowClick={(i, mods) => {
              const next = clickSelect(sel, order, i, mods);
              setSel(next);
              const row = rowAt(i);
              if (row && !mods.shift && !mods.toggle) void open(row);
              listRef.current?.focus();
            }}
            onBulk={(a, r) => act(a, r)}
            onClearSelection={() => setSel({ ids: current != null ? [current] : [], anchor: current })}
            onShowFolders={() => setView("folders")}
            searchRef={searchRef}
            listRef={listRef}
          />
        )}
        <ReadingPane
          thread={thread}
          views={views}
          expanded={expanded}
          focusId={current}
          theme={theme}
          onToggle={toggleExpanded}
          onBack={() => setView("list")}
          onCompose={replyTo}
          onAction={(a, r) => act(a, r)}
          onAllowRemote={(v, always) => {
            const done = always && v.summary.from ? api.allowSender(v.summary.from.email) : api.allowRemote(v.summary.id);
            void done.then(() => {
              for (const m of thread) if (views[m.id]) void loadView(m.id);
            });
          }}
          onAttachment={(a, how) => void attachment(a, how)}
          onRetry={(id) => void loadView(id)}
          onEditDraft={(v) => void startCompose(draftFromMessage(v, accountById(v.account_id), rich))}
        />
      </div>
      {compose && (
        <ComposeSheet
          draft={compose}
          accounts={accounts}
          error={composeError}
          sending={sending}
          onChange={setCompose}
          onSend={() => void send()}
          onSaveDraft={() => void (compose && saveDraft(compose, false).then((d) => setCompose((c) => (c ? { ...c, messageId: d.messageId } : c))))}
          onClose={() => void closeCompose()}
          onDiscard={() => void discardCompose()}
        />
      )}
      {settingsOpen && (
        <SettingsView
          settings={settings}
          accounts={accounts}
          info={info}
          section={settingsOpen}
          onSection={setSettingsOpen}
          onChange={updateSettings}
          onAccountsChanged={() =>
            void loadAccounts().then(({ all }) => {
              if (mailboxId != null && !Object.values(all).flat().some((b) => b.id === mailboxId)) setMailboxId(Object.values(all).flat().find((b) => b.role === "inbox")?.id ?? null);
            })
          }
          onClose={() => setSettingsOpen(null)}
        />
      )}
      {folderDialog?.kind === "create" && (
        <PromptDialog
          title={folderDialog.parent ? t("folders.newSubIn", { name: folderLabel(folderDialog.parent, t) }) : t("folders.new")}
          label={t("folders.name")}
          confirm={t("folders.create")}
          onDone={async (name) => {
            await api.createFolder(folderDialog.accountId, folderDialog.parent?.id ?? null, name);
            await reloadMailboxes(folderDialog.accountId);
          }}
          onClose={() => setFolderDialog(null)}
        />
      )}
      {folderDialog?.kind === "rename" && (
        <PromptDialog
          title={t("folders.renameTitle")}
          label={t("folders.name")}
          initial={folderLabel(folderDialog.mailbox, t)}
          confirm={t("folders.rename")}
          onDone={async (name) => {
            await api.renameFolder(folderDialog.mailbox.id, name);
            await reloadMailboxes(folderDialog.mailbox.account_id);
          }}
          onClose={() => setFolderDialog(null)}
        />
      )}
      {folderDialog?.kind === "delete" && (
        <ConfirmDialog
          title={t("folders.deleteTitle", { name: folderLabel(folderDialog.mailbox, t) })}
          text={t("folders.deleteText", { n: folderDialog.mailbox.total })}
          confirm={t("folders.delete")}
          danger
          onConfirm={() => {
            const m = folderDialog.mailbox;
            void api.deleteFolder(m.id).then(() => {
              void reloadMailboxes(m.account_id);
              if (mailboxId === m.id) setMailboxId(mailboxes[m.account_id]?.find((b) => b.role === "inbox")?.id ?? null);
            });
          }}
          onClose={() => setFolderDialog(null)}
        />
      )}
      {signIn && <SignInDialog account={signIn} onClose={() => setSignIn(null)} />}
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("read.move")} />}
      {preview && (
        <div className="overlay preview-overlay" onMouseDown={(e) => e.target === e.currentTarget && setPreview(null)} onKeyDown={(e) => e.key === "Escape" && setPreview(null)}>
          <figure className="preview" role="dialog" aria-modal="true" aria-label={preview.name}>
            <figcaption>
              <span>{preview.name}</span>
              <button type="button" className="icon-btn" autoFocus onClick={() => setPreview(null)} aria-label={t("common.close")}>
                <X size={16} strokeWidth={1.75} aria-hidden />
              </button>
            </figcaption>
            <img src={preview.url} alt={preview.name} />
          </figure>
        </div>
      )}
      <Toasts />
    </div>
  );
}

/** The password again after „Anmeldung erforderlich“. */
function SignInDialog({ account, onClose }: { account: AccountInfo; onClose: () => void }) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const problem = account.status?.error;
  const submit = async () => {
    try {
      await api.signIn(account.id, password);
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <Dialog
      title={t("signIn.title", { email: account.email })}
      description={problem ?? t("signIn.text")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={!password} onClick={() => void submit()}>
            {t("nav.signIn")}
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("setup.password")}</span>
        <input className="input" type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} onKeyDown={(e) => e.key === "Enter" && password && void submit()} />
      </label>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}
