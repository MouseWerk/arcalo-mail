import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  AlarmClock,
  Archive,
  BellRing,
  BookmarkMinus,
  BookmarkPlus,
  CalendarDays,
  CircleUserRound,
  NotebookPen,
  ListTodo,
  Clock,
  Crosshair,
  Download,
  FileText,
  Inbox,
  Layers,
  LogIn,
  MailMinus,
  MessagesSquare,
  Newspaper,
  Paperclip,
  Star,
  StarOff,
  Palette as PaletteIcon,
  PenSquare,
  Pin,
  PinOff,
  RefreshCw,
  Search,
  Send,
  Settings as SettingsIcon,
  ShieldAlert,
  Tags,
  Upload,
  UserPlus,
  X,
} from "lucide-react";
import { api, inApp, onEvent, onNotice, onOpenMessage, onReminders, onSaveDrafts, onTrayAction, openExternal, pickSavePath } from "./lib/api";
import { openCalendar, setWeekLocale, type CalendarRequest } from "./lib/calendar";
import { applyTheme, onSystemThemeChange, type Theme } from "./lib/theme";
import { cssHex, rememberSplash } from "./lib/splash";
import { currentLang, langFromLocale, setLang, useT } from "./lib/i18n";
import { committed, emptyDraft, fromComposeDraft, isEmpty, isValidAddress, reopened, replaceOwnText, replyDraft, textToEditorHtml, toDraft, type ComposeState } from "./lib/compose";
import { mailtoFromHref } from "./lib/mailto";
import { clickSelect, isControl, isTyping, keyAction, pagesFor, rangeGaps, type Selection } from "./lib/listing";
import { closeToast, runUndo, setUndo, showToast } from "./lib/toasts";
import { displayName, now as clockNow, setClock } from "./lib/format";
import { whenLabel } from "./lib/when";
import { SHORT_MAX, backStep, isTouch, layoutFor } from "./lib/touch";
import { applyTypeScale, onVisibility, tellNightMode, tellSystemBars, watchViewport } from "./lib/viewport";
import type {
  AccountInfo,
  AppInfo,
  Attachment,
  Category,
  ComposeDraft,
  InboxCounts,
  Mailbox,
  MessageSummary,
  MessageView,
  Mailto,
  MicrosoftSignIn,
  Notice,
  OutboxEntry,
  Page,
  Scheduled,
  Settings,
  Subscription,
  SwipeAction,
  ThemeChoice,
  TimeCounts,
  Undo,
  WaitingSender,
  UnsubscribePlan,
} from "./lib/types";
import { AccountSwitcher, stepAccount, switcherAccounts } from "./components/AccountSwitcher";
import { Sidebar, folderIcon, folderLabel, listedFolder, type FolderAction, type ViewEntry } from "./components/Sidebar";
import { MessageList, type BulkAction } from "./components/MessageList";
import { ReadingPane, type Loaded, type MarkAction, type MessageAction } from "./components/ReadingPane";
import { CATEGORIES, CATEGORY_ICON, CATEGORY_LABEL, InboxHead, InboxZero } from "./components/Inbox";
import { ScheduledView, ScreenerView, SubscriptionsView } from "./components/SmartViews";
import { WhenDialog, followupItems, snoozeItems } from "./components/TimeMenus";
import { ComposeSheet } from "./components/ComposeSheet";
import { CalendarView } from "./components/CalendarView";
import { AssistantBar } from "./components/Assistant";
import { AskView } from "./components/AskView";
import { AttachmentHub } from "./components/AttachmentHub";
import { ContactCardDialog } from "./components/ContactCard";
import { MAILTO_EVENT } from "./components/MailFrame";
import { Setup } from "./components/Setup";
import { SettingsView, exportDiagnostics, type SettingsSection } from "./components/Settings";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { ProblemCard } from "./components/Setup";
import { Toasts } from "./components/Toasts";
import { UpdateLayer, setBeforeRestart, startUpdates } from "./components/Updates";
import { Outbox } from "./components/Outbox";
import { ConfirmDialog, Dialog, LAYERS_EVENT, Menu, PromptDialog, closeTopLayer, openLayers, type MenuItem } from "./components/ui";
import { PassphraseDialog } from "./components/Meeting";

/** On a phone one pane is shown at a time; `view` says which (app.css, narrow layout). On a
 *  tablet `folders` opens the folders as a drawer over the list. */
type View = "folders" | "list" | "message";

const PAGE = 100;

/** Unread messages of a conversation expanded (and loaded) when it opens; the others on demand. */
const UNREAD_OPEN = 4;

/** The folders shown for all accounts together (the inboxes are the sorted inbox). */
type UnifiedRole = "drafts" | "sent" | "archive";
const UNIFIED_ROLES: UnifiedRole[] = ["drafts", "sent", "archive"];

type ListState = { key: string; total: number; pages: Record<number, MessageSummary[]>; loading: boolean };

type FolderDialog = FolderAction | null;

/** What the list pane shows: a folder (`mailboxId`), the sorted inbox of one account or of all,
 *  „Neue Absender“, the time tools' lists, the scheduled messages or the subscriptions. */
type Place =
  | { kind: "folder" }
  | { kind: "inbox"; account: number | null }
  /** One folder of every account at once: „Alle Entwürfe“, „Alle gesendeten“, „Alle archivierten“. */
  | { kind: "role"; role: UnifiedRole }
  | { kind: "screener" }
  | { kind: "snoozed" }
  | { kind: "later" }
  | { kind: "followups" }
  | { kind: "scheduled" }
  | { kind: "subscriptions" }
  | { kind: "ask" }
  | { kind: "files" }
  | { kind: "calendar" };

const VIEW_KEY = "arcalo-mail.view";

/** The lists kept for switching back at once, and how many of their first pages. */
const LIST_CACHE_SIZE = 12;
const LIST_CACHE_PAGES = 2;

/** How long notices of a sync are gathered before the lists are loaded again. */
const NOTICE_SETTLE_MS = 150;

/** Remembers the view on this device (browser storage: a convenience, may be gone). */
function rememberView(p: Place, mailbox: number | null) {
  try {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ p, mailbox }));
  } catch {
    // Private mode or full storage: the next start opens the inbox.
  }
}

/** The view of the last start when its account or folder is still there. */
function lastView(list: AccountInfo[], all: Record<number, Mailbox[]>): [Place, number | null] | null {
  let v: { p?: Place; mailbox?: number | null } | null = null;
  try {
    v = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "null");
  } catch {
    return null;
  }
  const p = v?.p;
  if (!p) return null;
  const box = (id: number | null | undefined) => (id == null ? null : (Object.values(all).flat().find((m) => m.id === id) ?? null));
  if (p.kind === "inbox") {
    if (p.account == null) return list.length > 1 ? [p, null] : null;
    const inbox = all[p.account]?.find((m) => m.role === "inbox");
    return inbox ? [p, inbox.id] : null;
  }
  if (p.kind === "role") return list.length > 1 && UNIFIED_ROLES.includes(p.role) ? [p, null] : null;
  if (p.kind === "folder") {
    const m = box(v?.mailbox);
    return m ? [p, m.id] : null;
  }
  return null;
}

/** A source of list pages, `key` names it (a new key starts the list afresh). */
type Source = { key: string; page: (offset: number, limit: number) => Promise<Page> };

/** A whole list as pages (the time tools' lists are short). */
const pages = (key: string, all: () => Promise<MessageSummary[]>): Source => ({
  key,
  page: async (offset, limit) => {
    const items = await all();
    return { total: items.length, offset, items: items.slice(offset, offset + limit) };
  },
});

/** The time picker of a time tool, open. */
type When = { kind: "snooze"; ids: number[] } | { kind: "followup"; ids: number[] } | { kind: "send"; followupSecs: number | null };

const NO_TIME: TimeCounts = { snoozed: 0, later: 0, followups: 0, scheduled: 0 };

/** How long after the last change the open message is saved as a draft. */
const AUTOSAVE_MS = 3000;

/** How long a discarded message can be brought back before its draft is removed. */
const DISCARD_UNDO_MS = 5000;

export function App() {
  const t = useT();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [theme, setTheme] = useState<Theme>("dark");
  const [accounts, setAccounts] = useState<AccountInfo[] | null>(null);
  const [mailboxes, setMailboxes] = useState<Record<number, Mailbox[]>>({});
  const [mailboxId, setMailboxId] = useState<number | null>(null);
  const [place, setPlace] = useState<Place>({ kind: "folder" });
  const [tab, setTab] = useState<Category>("people");
  const [counts, setCounts] = useState<InboxCounts | null>(null);
  const [screenerCount, setScreenerCount] = useState(0);
  const [times, setTimes] = useState<TimeCounts>(NO_TIME);
  const [waiting, setWaiting] = useState<WaitingSender[]>([]);
  const [subs, setSubs] = useState<Subscription[]>([]);
  const [unsubBusy, setUnsubBusy] = useState<string | null>(null);
  const [scheduled, setScheduled] = useState<Scheduled[]>([]);
  // A bundle of the newsletters tab opened as a stream of the sender's issues.
  const [stream, setStream] = useState<{ sender: string; name: string } | null>(null);
  const [when, setWhen] = useState<When | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; text: string; detail?: string; confirm: string; run: () => void } | null>(null);
  const [view, setView] = useState<View>("list");
  const [list, setList] = useState<ListState>({ key: "", total: 0, pages: {}, loading: true });
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MessageSummary[] | null>(null);
  // The switcher's numbers: unread mail in each account's inbox.
  const [acctCounts, setAcctCounts] = useState<Record<number, number>>({});
  // Search runs over every account; the switcher narrows the results to one (null: all).
  const [searchAccount, setSearchAccount] = useState<number | null>(null);
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
  const composeRef = useRef(compose);
  composeRef.current = compose;
  const [composeError, setComposeError] = useState<{ title: string; detail: string } | null>(null);
  // A message whose send was undone while another one is open: asked where it goes.
  const [takeBack, setTakeBack] = useState<ComposeState | null>(null);
  const [sending, setSending] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState<SettingsSection | null>(null);
  const [showOutbox, setShowOutbox] = useState(false);
  const [outbox, setOutbox] = useState<OutboxEntry[]>([]);
  const [folderDialog, setFolderDialog] = useState<FolderDialog>(null);
  const [signIn, setSignIn] = useState<AccountInfo | null>(null);
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const [palette, setPalette] = useState(false);
  const [preview, setPreview] = useState<{ name: string; url: string; text?: string } | null>(null);
  /** Asked before sending: the passphrase of the signing key, or sending a decrypted quote readable. */
  const [sendCheck, setSendCheck] = useState<{ kind: "unlock"; fingerprint: string; go: () => void } | { kind: "plain"; go: () => void } | null>(null);
  /** An attachment „Öffnen“ held back: a program or script (save only) or a macro document. */
  const [risky, setRisky] = useState<{ a: Attachment; risk: "macro" | "blocked" } | null>(null);
  const [contact, setContact] = useState<{ email: string; name: string | null } | null>(null);
  // What the calendar should open on (an appointment, a day, a new one's draft), once.
  const [calRequest, setCalRequest] = useState<CalendarRequest | null>(null);
  const [width, setWidth] = useState(() => (typeof window === "undefined" ? 1280 : window.innerWidth));
  const [height, setHeight] = useState(() => (typeof window === "undefined" ? 800 : window.innerHeight));
  // Phones: the settings' list of sections, before a section opens.
  const [settingsNav, setSettingsNav] = useState(true);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const openSeq = useRef(0);
  const openByIdSeq = useRef(0);
  const lastSync = useRef(Date.now());

  const touch = isTouch(info?.touch ?? false);
  const layout = layoutFor(width, touch);
  // A phone in landscape: two panes side by side, but too low for a floating sheet; writing and
  // the settings take the whole screen as on a phone.
  const short = touch && layout === "tablet" && height < SHORT_MAX;
  const fullScreen = layout === "phone" || short;

  const mailbox = useMemo(() => Object.values(mailboxes).flat().find((m) => m.id === mailboxId) ?? null, [mailboxes, mailboxId]);
  const threads = settings?.threads ?? true;
  const focus = settings?.focus ?? false;
  const bundles = settings?.bundles ?? true;
  const inboxIds = useMemo(() => new Set(Object.values(mailboxes).flat().filter((m) => m.role === "inbox").map((m) => m.id)), [mailboxes]);
  const multi = (accounts?.length ?? 0) > 1;
  /** The AI assistant is set up (Settings → KI). */
  const aiOn = (settings?.ai?.providers.length ?? 0) > 0;

  const source = useMemo((): Source | null => {
    switch (place.kind) {
      case "inbox": {
        const query = { account: place.account, tab, focus, threads, bundles };
        return { key: `inbox:${place.account}:${tab}:${focus}:${threads}:${bundles}`, page: (o, l) => api.inbox(query, o, l) };
      }
      case "snoozed":
        return pages("snoozed", api.snoozed);
      case "later":
        return pages("later", api.readLater);
      case "followups":
        return pages("followups", api.followups);
      case "folder":
        return mailboxId == null ? null : { key: `${mailboxId}:${threads}`, page: (o, l) => api.messages(mailboxId, o, l, threads) };
      case "role": {
        const role = place.role;
        return { key: `role:${role}`, page: (o, l) => api.roleList(role, o, l) };
      }
      default:
        return null;
    }
  }, [place, tab, focus, threads, bundles, mailboxId]);
  const sourceRef = useRef(source);
  sourceRef.current = source;
  const listKey = source?.key ?? "";

  // ---------------------------------------------------------------- start

  useEffect(() => {
    void Promise.all([api.appInfo(), api.settings()]).then(([i, s]) => {
      setClock(i.clock);
      setWeekLocale(i.locale);
      setInfo(i);
      setSettings(s);
    });
  }, []);

  // Updates: the desktop's checks and notices, the Android app's notice (components/Updates.tsx).
  const platform = info?.platform;
  useEffect(() => (platform ? startUpdates(platform) : undefined), [platform]);

  useEffect(() => {
    if (!settings || !info) return;
    const apply = () => setTheme(applyTheme(settings.theme));
    apply();
    setLang(settings.language === "system" ? langFromLocale(info.locale || navigator.language) : settings.language);
    return onSystemThemeChange(apply);
  }, [settings, info]);

  // The next start's opening animation: on or off, in the colors of the theme in use.
  useEffect(() => {
    if (!settings) return;
    rememberSplash({ off: !settings.startup_animation, bg: cssHex("--bg-app"), ink: cssHex("--text"), seal: cssHex("--accent") });
    tellSystemBars(theme !== "light", cssHex("--bg-app"));
    tellNightMode(settings.theme === "system" ? "system" : theme === "light" ? "light" : "dark");
  }, [settings?.startup_animation, settings?.theme, theme]);

  const loadAccounts = useCallback(async () => {
    const list = await api.accounts();
    setAccounts(list);
    const all: Record<number, Mailbox[]> = {};
    for (const a of list) all[a.id] = await api.mailboxes(a.id);
    setMailboxes(all);
    return { list, all };
  }, []);

  const placeRef = useRef(place);
  placeRef.current = place;
  const mailboxRef = useRef(mailboxId);
  mailboxRef.current = mailboxId;
  // Where the app opens was decided (before that, a folder list that arrives opens nothing).
  const startedRef = useRef(false);

  /** Where the app opens: all inboxes with two accounts or more (unless switched off), else the
   *  first account's inbox. */
  const openStart = useCallback((list: AccountInfo[], all: Record<number, Mailbox[]>, unified: boolean) => {
    const first = list.map((a) => all[a.id]?.find((m) => m.role === "inbox")).find(Boolean) ?? null;
    const [p, id]: [Place, number | null] =
      lastView(list, all) ??
      (list.length > 1 && unified ? [{ kind: "inbox", account: null }, null] : first ? [{ kind: "inbox", account: first.account_id }, first.id] : [{ kind: "folder" }, null]);
    // The refs at once: a folder list that arrives before the next render must not open another inbox.
    placeRef.current = p;
    mailboxRef.current = id;
    startedRef.current = true;
    setMailboxId(id);
    setPlace(p);
  }, []);

  useEffect(() => {
    void Promise.all([loadAccounts(), api.settings()]).then(([{ list, all }, s]) => openStart(list, all, s.unified_inbox));
    void api.outbox().then(setOutbox);
  }, []);

  useEffect(() => {
    if (settings && accounts && info) document.body.classList.add("ready");
  }, [settings, accounts, info]);

  // ---------------------------------------------------------------- phones and tablets

  useEffect(() => {
    const onResize = () => {
      setWidth(window.innerWidth);
      setHeight(window.innerHeight);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.toggleAttribute("data-touch", touch);
    if (!touch) return;
    applyTypeScale(root);
    return watchViewport(root);
  }, [touch]);

  // Touch screens: a long press opens the app's own menus (a folder's) or selects text in a
  // field; nothing else shows the WebView's stock context menu.
  useEffect(() => {
    if (!touch) return;
    const onMenu = (e: MouseEvent) => {
      if (e.defaultPrevented) return;
      if ((e.target as Element | null)?.closest?.("input, textarea, [contenteditable='true']")) return;
      e.preventDefault();
    };
    document.addEventListener("contextmenu", onMenu);
    return () => document.removeEventListener("contextmenu", onMenu);
  }, [touch]);

  // The layout on <html> too: what the app shows outside its panes (the setup, menus as bottom
  // sheets, dialogs) follows it (app.css, components/ui.tsx).
  useLayoutEffect(() => {
    document.documentElement.dataset.layout = layout;
  }, [layout]);

  // Hidden or shown again: a phone closes or opens its connections; back in front after a
  // while, the app checks for mail.
  useEffect(
    () =>
      onVisibility((visible) => {
        void api.setInFront(visible);
        if (visible && Date.now() - lastSync.current > 60_000) {
          lastSync.current = Date.now();
          void api.syncNow(null);
        }
      }),
    [],
  );

  const reloadMailboxes = useCallback(async (accountId: number) => {
    const boxes = await api.mailboxes(accountId);
    setMailboxes((all) => ({ ...all, [accountId]: boxes }));
    // The first sync brings the folders: open the inbox once it is there.
    const inbox = boxes.find((b) => b.role === "inbox");
    if (inbox && startedRef.current && placeRef.current.kind === "folder" && mailboxRef.current == null) {
      setMailboxId(inbox.id);
      setPlace({ kind: "inbox", account: inbox.account_id });
    }
  }, []);

  /** The tabs' numbers, „Neue Absender“ and the time tools' counts. */
  // Only the answers of the latest round count: an older one arriving late (a sync's reload
  // that started before a snooze) must not put back counts that changed since.
  const smartSeq = useRef(0);
  const reloadSmart = useCallback(() => {
    const p = placeRef.current;
    const seq = ++smartSeq.current;
    const latest = <T,>(f: (v: T) => void) => (v: T) => {
      if (seq === smartSeq.current) f(v);
    };
    void api.inboxCounts(p.kind === "inbox" ? p.account : null).then(
      latest((c) => {
        setCounts(c);
        if (p.kind !== "inbox" || p.account == null) setScreenerCount(c.screener);
      }),
    );
    if (p.kind === "inbox" && p.account != null) void api.inboxCounts(null).then(latest((c) => setScreenerCount(c.screener)));
    void api.timeCounts().then(latest(setTimes));
    void api.accountCounts().then(latest((list) => setAcctCounts(Object.fromEntries(list.map((c) => [c.account_id, c.unread])))));
    if (p.kind === "screener") void api.screenerSenders(null).then(latest(setWaiting));
    if (p.kind === "subscriptions") void api.subscriptions().then(latest(setSubs));
    if (p.kind === "scheduled") void api.scheduled().then(latest(setScheduled));
  }, []);

  useEffect(() => {
    if (accounts) reloadSmart();
  }, [place, accounts?.length, settings?.screener, settings?.focus]);

  // ---------------------------------------------------------------- the list

  // Pages on their way: each is asked for once while the list scrolls over it.
  const inflight = useRef(new Set<string>());
  const fetchPage = useCallback(async (key: string, page: number, again = false) => {
    const src = sourceRef.current;
    if (!src || src.key !== key) return;
    const id = `${key}#${page}`;
    // `again`: after a change the page is asked for anew, also while an older answer is on its way.
    if (inflight.current.has(id) && !again) return;
    if (!again) inflight.current.add(id);
    try {
      const r = await src.page(page * PAGE, PAGE);
      setList((l) => (l.key === key ? { ...l, total: r.total, pages: { ...l.pages, [page]: r.items }, loading: false } : l));
    } finally {
      if (!again) inflight.current.delete(id);
    }
  }, []);

  /** Loads list pages now (into the list too) and returns them. */
  const loadPages = async (pages: number[]): Promise<Record<number, MessageSummary[]>> => {
    const src = sourceRef.current;
    if (!src) return {};
    const got = await Promise.all(pages.map(async (p) => [p, (await src.page(p * PAGE, PAGE)).items] as const));
    const loaded = Object.fromEntries(got);
    setList((l) => (l.key === src.key ? { ...l, pages: { ...l.pages, ...loaded } } : l));
    return loaded;
  };

  const refreshList = useCallback(() => {
    setList((l) => {
      const pages = Object.keys(l.pages).map(Number);
      for (const p of pages.length ? pages : [0]) void fetchPage(l.key, p, true);
      return l;
    });
  }, [fetchPage]);

  // Before the paint: a list seen before shows in the same frame as the click that asked for it.
  useLayoutEffect(() => {
    // A place without a list (Dateien, Frag dein Postfach, Neue Absender, Abos, Geplant): the
    // last folder's rows go, so no key works on a list that is not shown.
    if (!listKey) {
      setList((l) => ({ key: "", total: 0, pages: {}, loading: l.loading }));
      setSel({ ids: [], anchor: null });
      return;
    }
    // A list seen a moment ago (another tab, account or folder) shows at once from its last
    // pages and is brought up to date behind them; a new one starts empty.
    const seen = listCache.current.get(listKey);
    setList(seen ? { ...seen, loading: false } : { key: listKey, total: 0, pages: {}, loading: true });
    setSel({ ids: [], anchor: null });
    setCurrent(null);
    setThread([]);
    setStream(null);
    for (const p of seen ? Object.keys(seen.pages).map(Number) : [0]) void fetchPage(listKey, p, !!seen);
  }, [listKey]);

  // The lists seen last, their first pages: for switching back without waiting.
  const listCache = useRef(new Map<string, ListState>());
  useEffect(() => {
    if (!list.key || list.loading) return;
    const cache = listCache.current;
    const first = Object.fromEntries(Object.entries(list.pages).filter(([p]) => Number(p) < LIST_CACHE_PAGES));
    cache.delete(list.key);
    cache.set(list.key, { ...list, pages: first });
    while (cache.size > LIST_CACHE_SIZE) cache.delete(cache.keys().next().value as string);
  }, [list]);

  const onRange = useCallback(
    (start: number, end: number) => {
      if (results || !listKey) return;
      // Half a page ahead and behind: the next rows are there before the list reaches them.
      for (const p of pagesFor(Math.max(0, start - PAGE / 2), Math.min(list.total || end, end + PAGE / 2), PAGE)) if (!list.pages[p]) void fetchPage(list.key, p);
    },
    [results, listKey, list.pages, list.key, fetchPage],
  );

  const shown = useMemo(() => (results && searchAccount != null ? results.filter((m) => m.account_id === searchAccount) : results), [results, searchAccount]);
  const rowAt = useCallback(
    (i: number): MessageSummary | undefined => (shown ? shown[i] : list.pages[Math.floor(i / PAGE)]?.[i % PAGE]),
    [shown, list.pages],
  );
  const total = shown ? shown.length : list.total;
  const order = useMemo(() => Array.from({ length: total }, (_, i) => rowAt(i)?.id), [total, rowAt]);

  // Search as you type, over all accounts (local index, works offline). An answer counts only
  // while its query is still the one in the field.
  const queryRef = useRef(query);
  queryRef.current = query;
  const stillAsked = (q: string) => queryRef.current.trim() === q;
  useEffect(() => {
    const q = query.trim();
    setServer({ busy: false, done: false });
    serverHits.current = new Set();
    if (!q) {
      setResults(null);
      setSearchAccount(null);
      return;
    }
    let alive = true;
    const timer = setTimeout(
      () =>
        void api.search(q, null).then((r) => {
          if (alive && stillAsked(q)) setResults(r);
        }),
      120,
    );
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [query]);

  // The store changed under open results: search again, keeping what the server found.
  useEffect(() => {
    const q = query.trim();
    if (!q || searchRev === 0) return;
    let alive = true;
    const timer = setTimeout(
      () =>
        void api.search(q, null).then((local) =>
          setResults((r) => {
            if (!r || !alive || !stillAsked(q)) return r;
            const known = new Set(local.map((m) => m.id));
            return [...local, ...r.filter((m) => serverHits.current.has(m.id) && !known.has(m.id))];
          }),
        ),
      300,
    );
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [searchRev]);

  const serverSearch = async () => {
    const accountId = mailbox?.account_id ?? accounts?.[0]?.id;
    const q = query.trim();
    if (accountId == null || !q) return;
    setServer({ busy: true, done: false });
    try {
      const more = await api.serverSearch(accountId, q);
      // The query changed meanwhile: these hits belong to no search on screen.
      if (!stillAsked(q)) return;
      for (const m of more) serverHits.current.add(m.id);
      setResults((r) => {
        if (!r) return r;
        const known = new Set(r.map((m) => m.id));
        return [...r, ...more.filter((m) => !known.has(m.id))];
      });
      setServer({ busy: false, done: true });
    } catch (e) {
      if (!stillAsked(q)) return;
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

  /** Opens a message with its conversation. `byId`: a message known by its id only, whose row
   *  does not say how long its conversation is. */
  const open = useCallback(
    async (m: MessageSummary, byId = false) => {
      const seq = ++openSeq.current;
      setCurrent(m.id);
      setView("message");
      // A bundle: the sender's issues one below the other, the newest first and open.
      const bundle = (m.bundle ?? 0) > 0 && !results && placeRef.current.kind === "inbox" && m.from;
      const p = placeRef.current;
      const list = bundle && m.from ? await api.bundle(p.kind === "inbox" ? p.account : null, m.from.email) : threads && (m.thread_count > 1 || byId) ? await api.thread(m.thread_id) : [m];
      if (seq !== openSeq.current) return;
      const th = list.length ? list : [m];
      setStream(bundle && m.from ? { sender: m.from.email, name: displayName(m.from, currentLang()) } : null);
      setThread(th);
      // A long conversation loads only what it shows: its newest unread messages and the newest
      // one; the others when they are expanded.
      const open = bundle
        ? th.slice(0, 3).map((x) => x.id)
        : [...new Set([...th.filter((x) => !x.seen).map((x) => x.id).slice(-UNREAD_OPEN), th[th.length - 1].id])];
      setExpanded(open);
      setViews({});
      await Promise.all(open.map((id) => loadView(id)));
      if (seq !== openSeq.current) return;
      const unread = th.filter((x) => !x.seen && (!bundle || open.includes(x.id))).map((x) => x.id);
      const after = settings?.mark_read_after_secs;
      if (unread.length && after != null) {
        const mark = () => {
          if (seq !== openSeq.current) return;
          void api.setFlags(unread, true, null);
          patchRows(unread, { seen: true });
          if (m.thread_count > 1 || bundle) patchRows([m.id], { seen: true });
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
    [threads, settings, loadView, patchRows, results],
  );

  const toggleExpanded = (id: number) => {
    setExpanded((e) => (e.includes(id) ? e.filter((x) => x !== id) : [...e, id]));
    if (!views[id]) void loadView(id);
  };

  // ---------------------------------------------------------------- actions

  /** What an action applies to: the selection, else the open conversation in this folder. */
  /** Whether a message belongs to what the list shows (a conversation reaches into other folders). */
  const listed = (m: MessageSummary) => {
    if (results || stream) return true;
    switch (place.kind) {
      case "folder":
        return m.mailbox_id === mailboxId;
      case "inbox":
        return inboxIds.has(m.mailbox_id) && (place.account == null || m.account_id === place.account);
      default:
        return true;
    }
  };

  const targets = (): number[] => {
    if (sel.ids.length > 1) return sel.ids;
    if (!thread.length) return current != null ? [current] : [];
    const here = thread.filter(listed).map((m) => m.id);
    return here.length ? here : current != null ? [current] : [];
  };

  // The list as it is now, for what finishes later (an archive that waited for the backend).
  const listNow = useRef({ current, order, rowAt });
  listNow.current = { current, order, rowAt };

  /** After rows went away: the next one opens (the one below, else above), if the open message
   *  was among them; read when the removal finished, not when it started. */
  const afterRemoval = (gone: number[]) => {
    const { current, order, rowAt } = listNow.current;
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
    reloadSmart();
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
      .filter((b) => listedFolder(b) && b.id !== box.id && b.role !== "drafts")
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

  // ---------------------------------------------------------------- time tools and the sorted inbox

  const point = (anchor?: DOMRect) => (anchor ? { x: anchor.left, y: anchor.bottom + 4 } : { x: Math.max(16, width - 320), y: 120 });

  /** The views of the open conversation, the newest loaded first. */
  const loadedViews = (): MessageView[] =>
    thread
      .map((m) => views[m.id])
      .filter((v): v is MessageView => !!v && typeof v === "object" && "summary" in v)
      .reverse();
  const reloadViews = () => {
    for (const m of thread) if (views[m.id]) void loadView(m.id);
  };

  const snooze = async (ids: number[], at: number) => {
    try {
      await api.snooze(ids, at);
    } catch (e) {
      showToast({ title: t("snooze.snoozeFailed"), detail: String(e), tone: "danger" });
      return;
    }
    afterRemoval(ids);
    reloadCounts();
    const back = async () => {
      await api.unsnooze(ids);
      refreshList();
      reloadCounts();
      showToast({ title: t("snooze.back"), duration: 3000 });
    };
    setUndo(() => void back());
    showToast({ title: t("snooze.done", { when: whenLabel(at, clockNow(), currentLang()) }), action: { label: t("common.undo"), run: () => void back() }, duration: 5000 });
  };

  const snoozeMenu = (ids: number[], anchor?: DOMRect) =>
    setMenu({ items: snoozeItems(t, (at) => void snooze(ids, at), () => setWhen({ kind: "snooze", ids })), at: point(anchor) });

  const setPinned = async (ids: number[], on: boolean) => {
    await api.setPinned(ids, on);
    patchRows(ids, { pinned: on });
    refreshList();
    reloadViews();
    reloadSmart();
    showToast({ title: on ? t("toast.pinned") : t("toast.unpinned"), duration: 2500 });
  };

  const setLater = async (ids: number[], on: boolean) => {
    await api.setReadLater(ids, on);
    reloadViews();
    reloadSmart();
    if (place.kind === "later") refreshList();
    showToast({ title: on ? t("toast.later") : t("toast.notLater"), duration: 2500 });
  };

  const setFollowup = async (ids: number[], at: number) => {
    try {
      await api.setFollowup(ids, at);
      reloadViews();
      showToast({ title: t("read.followupAt", { when: whenLabel(at, clockNow(), currentLang()) }), duration: 3500 });
    } catch (e) {
      showToast({ title: t("toast.actionFailed"), detail: String(e), tone: "danger" });
    }
  };

  const dismissFollowups = async (ids: number[]) => {
    await api.dismissFollowups(ids);
    reloadViews();
    reloadSmart();
    if (place.kind === "followups") afterRemoval(ids);
    showToast({ title: t("toast.followupOff"), duration: 2500 });
  };

  const moveSender = async (email: string, name: string, c: Category | null) => {
    await api.setSenderCategory(email, c);
    refreshList();
    reloadSmart();
    if (c) showToast({ title: t("inbox.movedSender", { name, tab: t(CATEGORY_LABEL[c]) }), duration: 3500 });
  };

  /** „Abbestellen“: asks first (one click and e-mail), then does it; a list that only has a web
   *  page asks before it opens. */
  /**
   * „Abbestellen“ on a message: the backend reads its `List-Unsubscribe`, the user sees exactly
   * what happens (the address the e-mail goes to, the address of the one click, the page) and
   * confirms; the backend then does only that.
   */
  const unsubscribe = async (messageId: number, name: string) => {
    let plan: UnsubscribePlan;
    try {
      plan = await api.unsubscribePlan(messageId);
    } catch (e) {
      showToast({ title: t("subs.unsubscribeFailed"), detail: String(e), tone: "danger" });
      return;
    }
    const m = plan.method;
    const run = async () => {
      setUnsubBusy(plan.sender);
      try {
        const r = await api.unsubscribe(messageId, m);
        if (r.kind === "link") {
          await openExternal(r.url);
          await api.unsubscribedByLink(plan.sender);
        } else showToast({ title: r.kind === "done" ? t("subs.done", { name }) : t("subs.sent", { to: r.to }), tone: "success", duration: 4000 });
        reloadSmart();
        reloadViews();
      } catch (e) {
        showToast({ title: t("subs.unsubscribeFailed"), detail: String(e), tone: "danger" });
      } finally {
        setUnsubBusy(null);
      }
    };
    if (m.kind === "link") setConfirm({ title: t("subs.linkTitle"), text: t("subs.confirmLink"), detail: m.url, confirm: t("subs.openPage"), run: () => void run() });
    else if (m.kind === "mailto")
      setConfirm({ title: t("subs.confirmTitle", { name }), text: t("subs.confirmMailtoTo", { to: m.to, subject: m.subject }), confirm: t("subs.unsubscribe"), run: () => void run() });
    else setConfirm({ title: t("subs.confirmTitle", { name }), text: t("subs.confirmOneClick"), detail: m.url, confirm: t("subs.unsubscribe"), run: () => void run() });
  };

  const moreMenu = (anchor?: DOMRect) => {
    const ids = targets();
    const latest = loadedViews()[0] ?? null;
    if (!ids.length || !latest) return;
    const at = point(anchor);
    const focusView = (current != null ? (views[current] as MessageView | undefined) : undefined) ?? latest;
    const sender = focusView && "summary" in focusView ? focusView.summary.from : null;
    const row = current != null ? rowAt(order.indexOf(current)) : undefined;
    const pinned = latest.marks.pinned;
    const later = latest.marks.later;
    const important = thread.some((m) => ids.includes(m.id) && m.important) || !!row?.important;
    const items: MenuItem[] = [
      {
        label: important ? t("read.notImportant") : t("read.important"),
        icon: important ? <StarOff size={14} aria-hidden /> : <Star size={14} aria-hidden />,
        onSelect: () =>
          void api.setImportant(ids, !important).then(() => {
            patchRows(ids, { important: !important });
          }),
      },
      { label: pinned ? t("read.unpin") : t("read.pin"), icon: pinned ? <PinOff size={14} aria-hidden /> : <Pin size={14} aria-hidden />, onSelect: () => void setPinned(ids, !pinned) },
      {
        label: later ? t("read.notLater") : t("read.later"),
        icon: later ? <BookmarkMinus size={14} aria-hidden /> : <BookmarkPlus size={14} aria-hidden />,
        onSelect: () => void setLater(ids, !later),
      },
      {
        label: t("read.followup"),
        icon: <AlarmClock size={14} aria-hidden />,
        onSelect: () =>
          setMenu({
            items: followupItems(
              t,
              null,
              (days) => void setFollowup([latest.summary.id], Math.floor(clockNow().getTime() / 1000) + days * 86400),
              () => setWhen({ kind: "followup", ids: [latest.summary.id] }),
              latest.marks.followup_due != null ? () => void dismissFollowups([latest.summary.id]) : null,
            ),
            at,
          }),
      },
    ];
    if (sender) {
      const name = displayName(sender, currentLang());
      items.push({
        label: t("read.moveSender"),
        icon: <Tags size={14} aria-hidden />,
        onSelect: () =>
          setMenu({
            items: [
              { heading: t("read.moveSender") },
              ...CATEGORIES.map((c): MenuItem => {
                const Icon = CATEGORY_ICON[c];
                return { label: t(CATEGORY_LABEL[c]), icon: <Icon size={14} aria-hidden />, checked: row?.category ? row.category === c : undefined, onSelect: () => void moveSender(sender.email, name, c) };
              }),
              "separator",
              { label: t("inbox.asSorted"), onSelect: () => void moveSender(sender.email, name, null) },
            ],
            at,
          }),
      });
    }
    const listView = loadedViews().find((v) => v.unsubscribe);
    if (listView?.summary.from) {
      const from = listView.summary.from;
      items.push("separator", { label: t("read.unsubscribe"), icon: <MailMinus size={14} aria-hidden />, onSelect: () => void unsubscribe(listView.summary.id, displayName(from, currentLang())) });
    }
    const target = focusView && "summary" in focusView ? focusView.summary.id : latest.summary.id;
    items.push(
      "separator",
      { heading: t("arcalo.heading") },
      { label: t("arcalo.page"), icon: <NotebookPen size={14} aria-hidden />, onSelect: () => void sendToArcalo(target, "page") },
      { label: t("arcalo.task"), icon: <ListTodo size={14} aria-hidden />, onSelect: () => void sendToArcalo(target, "task") },
    );
    if (touch) items.push("separator", { label: t("read.spam"), icon: <ShieldAlert size={14} aria-hidden />, onSelect: () => act("spam") });
    setMenu({ items, at });
  };

  /** „An Arcalo senden“: a page or a task in Arcalo; without Arcalo the page as a Markdown file. */
  const sendToArcalo = async (id: number, target: "page" | "task") => {
    try {
      const r = await api.sendToArcalo(id, target);
      if (r.via_cli) {
        showToast({ title: t(target === "page" ? "arcalo.sentPage" : "arcalo.sentTask"), detail: r.title, tone: "success", duration: 4000 });
        return;
      }
      const path = await pickSavePath(r.file_name ?? "mail.md");
      if (!path) return;
      await api.saveMarkdown(path, r.markdown ?? "");
      showToast({ title: t("arcalo.saved"), detail: t("arcalo.noCli"), duration: 5000, action: undefined });
    } catch (e) {
      showToast({ title: t("arcalo.failed"), detail: String(e), tone: "danger" });
    }
  };

  const onMark = (a: MarkAction, v: MessageView) => {
    const ids = [v.summary.id];
    if (a === "unpin") void setPinned(targets(), false);
    else if (a === "removeFollowup") void dismissFollowups(ids);
    else
      void api.unsnooze(ids).then(() => {
        refreshList();
        reloadCounts();
        reloadViews();
        showToast({ title: t("snooze.back"), duration: 3000 });
      });
  };

  const act = (a: MessageAction | BulkAction, anchor?: DOMRect) => {
    const ids = targets();
    if (!ids.length) return;
    const rows = [...thread, ...(results ?? []), ...Object.values(list.pages).flat()].filter((m) => ids.includes(m.id));
    switch (a) {
      case "snooze":
        snoozeMenu(ids, anchor);
        break;
      case "more":
        moreMenu(anchor);
        break;
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

  // Saving drafts. Saves run one after the other, and each one after the first replaces the
  // draft the first one stored (its Message-ID), so two saves at once never leave two drafts.
  const saveChain = useRef<Promise<unknown>>(Promise.resolve());
  const savedIds = useRef(new Map<number, string>());
  // What was saved last, per sheet: a save of the same text is not repeated.
  const savedAs = useRef(new Map<number, string>());
  const [, setSavedTick] = useState(0);
  const snapshot = (d: ComposeState) => JSON.stringify(toDraft({ ...d, messageId: null }));
  const isSaved = (d: ComposeState) => savedAs.current.get(d.uid) === snapshot(d);

  /** Stores the sheet in Drafts; resolves to its Message-ID. */
  const persist = (d: ComposeState): Promise<string> => {
    const run = saveChain.current.then(async () => {
      const known = savedIds.current.get(d.uid) ?? d.messageId;
      const snap = snapshot(d);
      if (known && savedAs.current.get(d.uid) === snap) return known;
      const messageId = await api.saveDraft(toDraft({ ...d, messageId: known }));
      savedIds.current.set(d.uid, messageId);
      savedAs.current.set(d.uid, snap);
      setSavedTick((n) => n + 1);
      setCompose((c) => (c && c.uid === d.uid && c.messageId !== messageId ? { ...c, messageId } : c));
      return messageId;
    });
    saveChain.current = run.catch(() => {});
    return run;
  };

  /** Saves the sheet as a draft; `false` when that failed (the sheet then says why, unless `quiet`). */
  const saveDraft = async (d: ComposeState, quiet: boolean): Promise<boolean> => {
    if (d.accountId == null) return true;
    try {
      await persist(d);
      if (!quiet) showToast({ title: t("toast.draftSaved"), duration: 2500 });
      return true;
    } catch (e) {
      if (!quiet) {
        setComposeError({ title: t("toast.draftFailed"), detail: String(e) });
        showToast({ title: t("toast.draftFailed"), detail: String(e), tone: "danger" });
      }
      return false;
    }
  };

  /** Saves the open message unless it is empty or saved as it is. */
  const flushDraft = async () => {
    const c = composeRef.current;
    if (c && !isEmpty(c) && !isSaved(c)) await saveDraft(c, true);
  };

  /** Opens `next` in the sheet; an open message is saved first and stays when that fails. */
  const startCompose = async (next: ComposeState): Promise<boolean> => {
    const open = composeRef.current;
    if (open && open.uid !== next.uid && !isEmpty(open) && !(await saveDraft(open, false))) return false;
    setComposeError(null);
    setCompose(next);
    return true;
  };

  /** A message that comes back to the sheet (the undo of a send, of „Später senden“, of
   *  „Verwerfen“): straight in when the sheet is free, else the user decides. */
  const bringBack = (state: ComposeState) => {
    const back = reopened(state);
    const open = composeRef.current;
    if (open && !isEmpty(open)) setTakeBack(back);
    else {
      setComposeError(null);
      setCompose(back);
    }
  };

  const newMessage = (to: string[] = []) => {
    // From the account in view (its inbox or folder, or the one the switcher narrowed to).
    const here = mailbox?.account_id ?? (place.kind === "inbox" ? place.account : null) ?? (results ? searchAccount : null);
    const account = accountById(here) ?? accounts?.[0] ?? null;
    void startCompose(emptyDraft(account, rich, to));
  };

  /** A stored message in the sheet (a scheduled one taken back, a draft from Drafts), with its
   *  files: those on disk that are still there, and the stored attachments it forwards. */
  const editStored = async (d: ComposeDraft) => {
    const [files, forwarded] = await Promise.all([
      d.attachments.length ? api.fileInfos(d.attachments).catch(() => []) : Promise.resolve([]),
      d.forwarded.length ? api.attachments(d.forwarded).catch(() => []) : Promise.resolve([]),
    ]);
    const missing = d.attachments.length - files.length + (d.forwarded.length - forwarded.length);
    if (missing > 0) showToast({ title: t("compose.filesMissing", { n: missing }), tone: "danger", duration: 8000 });
    await startCompose(fromComposeDraft(d, files, forwarded));
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

  /** Whether the sheet's recipients can be sent to (the error shows in the sheet otherwise). */
  const recipientsOk = (c: ComposeState): boolean => {
    const all = [...c.to, ...c.cc, ...c.bcc];
    if (!all.length) {
      setComposeError({ title: t("compose.sendFailed"), detail: t("compose.noRecipient") });
      return false;
    }
    const bad = all.find((a) => !isValidAddress(a));
    if (bad) {
      setComposeError({ title: t("compose.sendFailed"), detail: t("compose.invalidNamed", { address: bad }) });
      return false;
    }
    return true;
  };

  /** „Später senden“: the message waits here until `at`. */
  const sendLater = async (at: number, followupSecs: number | null, confirmedPlain = false) => {
    const open = composeRef.current;
    if (!open || sending) return;
    const state = committed(open);
    if (state !== open) setCompose(state);
    if (!recipientsOk(state)) return;
    if (!(await readyToSend(state, () => void sendLater(at, followupSecs, true), confirmedPlain))) return;
    setSending(true);
    setComposeError(null);
    try {
      await saveChain.current;
      const id = await api.sendLater(toDraft({ ...state, messageId: savedIds.current.get(state.uid) ?? state.messageId }), at, followupSecs);
      setCompose((c) => (c && c.uid === state.uid ? null : c));
      reloadSmart();
      showToast({
        title: t("scheduled.done", { when: whenLabel(at, clockNow(), currentLang()) }),
        duration: 5000,
        action: {
          label: t("common.undo"),
          run: () =>
            void api.unschedule(id).then(() => {
              bringBack(state);
              reloadSmart();
            }),
        },
      });
    } catch (e) {
      setComposeError({ title: t("compose.sendFailed"), detail: String(e) });
    } finally {
      setSending(false);
    }
  };

  /**
   * Before a message goes out: a decrypted message's quote is not sent readable without a yes,
   * and an own OpenPGP key that signs it gets its passphrase first. `false`: asked, wait.
   */
  const readyToSend = async (state: ComposeState, go: () => void, confirmedPlain = false) => {
    if (state.fromEncrypted && !state.protect?.encrypt && !confirmedPlain) {
      setSendCheck({ kind: "plain", go });
      return false;
    }
    if (state.protect?.method === "pgp" && state.protect.sign && state.accountId != null) {
      const fp = await api.pgpLocked(state.accountId).catch(() => null);
      if (fp) {
        setSendCheck({ kind: "unlock", fingerprint: fp, go });
        return false;
      }
    }
    return true;
  };

  const send = async (followupSecs: number | null = null, confirmedPlain = false) => {
    const open = composeRef.current;
    if (!open || sending) return;
    // Addresses still being typed go along.
    const state = committed(open);
    if (state !== open) setCompose(state);
    if (!recipientsOk(state)) return;
    if (!(await readyToSend(state, () => void send(followupSecs, true), confirmedPlain))) return;
    setSending(true);
    setComposeError(null);
    try {
      // A save in flight first: the message replaces the draft it stored.
      await saveChain.current;
      const q = await api.send(toDraft({ ...state, messageId: savedIds.current.get(state.uid) ?? state.messageId }), followupSecs);
      setCompose((c) => (c && c.uid === state.uid ? null : c));
      const takeBackSent = () =>
        void api.cancelSend(q.outbox_id, q.message_id).then((ok) => {
          if (ok) {
            bringBack(state);
            showToast({ title: t("toast.sendCancelled"), duration: 3000 });
          } else showToast({ title: t("toast.alreadySent"), tone: "danger", duration: 3000 });
        });
      const toastId = showToast({
        title: q.delay > 0 ? t("toast.sending") : t("toast.queued"),
        duration: q.delay > 0 ? q.delay * 1000 : 3000,
        action: q.delay > 0 ? { label: t("common.undo"), run: takeBackSent } : undefined,
      });
      setUndo(() => {
        closeToast(toastId);
        takeBackSent();
      }, q.delay * 1000);
      void api.outbox().then(setOutbox);
    } catch (e) {
      setComposeError({ title: t("compose.sendFailed"), detail: String(e) });
    } finally {
      setSending(false);
    }
  };

  /** Esc, the close button, Back: the message is saved as a draft first; the sheet closes only
   *  when that worked (otherwise it stays with the reason). */
  const closeCompose = async () => {
    const open = composeRef.current;
    if (!open) return;
    const d = committed(open);
    if (!isEmpty(d) && !(await saveDraft(d, false))) return;
    // Emptied after it was saved: the saved draft goes too.
    if (isEmpty(d)) {
      await saveChain.current;
      const id = savedIds.current.get(d.uid);
      if (id && d.accountId != null) void api.discardDraft(d.accountId, id).catch(() => {});
    }
    setComposeError(null);
    setCompose((c) => (c && c.uid === d.uid ? null : c));
  };

  /** „Verwerfen“: the sheet closes at once; the saved draft is removed after the undo time. */
  const discardCompose = () => {
    const d = composeRef.current;
    if (!d) return;
    setCompose(null);
    setComposeError(null);
    let undone = false;
    const timer = setTimeout(() => {
      if (undone) return;
      void saveChain.current.then(() => {
        const id = savedIds.current.get(d.uid) ?? d.messageId;
        if (id && d.accountId != null) void api.discardDraft(d.accountId, id);
      });
    }, DISCARD_UNDO_MS);
    const back = () => {
      if (undone) return;
      undone = true;
      clearTimeout(timer);
      bringBack(d);
    };
    setUndo(back, DISCARD_UNDO_MS);
    showToast({ title: t("toast.draftDiscarded"), duration: DISCARD_UNDO_MS, action: { label: t("common.undo"), run: back } });
  };

  // An e-mail link in a message: the compose sheet with what the link names (as the system's
  // e-mail links: addresses, Cc, Bcc, subject and text).
  useEffect(() => {
    const onMailto = (e: Event) => {
      const m = mailtoFromHref(String((e as CustomEvent).detail));
      if (m) composeMailto(m);
    };
    window.addEventListener(MAILTO_EVENT, onMailto);
    return () => window.removeEventListener(MAILTO_EVENT, onMailto);
  });

  // ---------------------------------------------------------------- attachments

  const attachment = async (a: Attachment, how: "open" | "open_anyway" | "save" | "preview") => {
    const name = a.filename ?? t("read.unnamed");
    try {
      if (how === "preview") {
        const p = await api.previewAttachment(a.id);
        if (p.mime_type.startsWith("text/")) {
          const bytes = Uint8Array.from(atob(p.data_b64), (c) => c.charCodeAt(0));
          setPreview({ name, url: "", text: new TextDecoder().decode(bytes) });
        } else setPreview({ name, url: `data:${p.mime_type};base64,${p.data_b64}` });
      } else if (how === "open" || how === "open_anyway") {
        const r = await api.openAttachment(a.id, how === "open_anyway");
        if (!r.opened && r.risk !== "none") setRisky({ a, risk: r.risk });
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

  // A sync reports in batches, several a second while a large account catches up: what they ask
  // for is gathered and done once a moment later (one reload per account, one of the list),
  // instead of a round of reloads and renders per batch.
  const settle = useRef<{ accounts: Set<number>; list: boolean; search: boolean; timer: number | null }>({ accounts: new Set(), list: false, search: false, timer: null });
  const settleWith = useRef({ reloadMailboxes, reloadSmart, refreshList });
  settleWith.current = { reloadMailboxes, reloadSmart, refreshList };
  const settleSoon = (account: number, list: boolean, search: boolean) => {
    const s = settle.current;
    s.accounts.add(account);
    s.list ||= list;
    s.search ||= search;
    if (s.timer != null) return;
    s.timer = window.setTimeout(() => {
      const { accounts: ids, list: l, search: q } = settle.current;
      settle.current = { accounts: new Set(), list: false, search: false, timer: null };
      const f = settleWith.current;
      for (const id of ids) void f.reloadMailboxes(id);
      f.reloadSmart();
      if (q) setSearchRev((v) => v + 1);
      if (l) f.refreshList();
    }, NOTICE_SETTLE_MS);
  };
  useEffect(() => () => window.clearTimeout(settle.current.timer ?? undefined), []);

  useEffect(
    () =>
      onNotice((n: Notice) => {
        switch (n.kind) {
          case "status":
            setAccounts((list) => list?.map((a) => (a.id === n.account_id ? { ...a, status: n } : a)) ?? list);
            break;
          case "changed":
            settleSoon(n.account_id, !n.mailboxes.length || placeRef.current.kind !== "folder" || (mailboxId != null && n.mailboxes.includes(mailboxId)), true);
            break;
          case "new_mail":
            settleSoon(n.account_id, false, false);
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
    [mailboxId, refreshList, reloadMailboxes, reloadSmart],
  );

  // Reminders without an answer: a note with the way to them.
  useEffect(
    () =>
      onReminders((list) => {
        for (const r of list)
          showToast({ title: t("toast.reminder", { subject: r.subject || t("list.noSubject") }), duration: 8000, action: { label: t("toast.show"), run: () => goPlace({ kind: "followups" }) } });
        reloadSmart();
        if (placeRef.current.kind === "followups") refreshList();
      }),
    [refreshList, reloadSmart],
  );

  // The tray menu's „Neue Nachricht“.
  // Subscribed once (each subscription is a round trip to the shell).
  const newMessageRef = useRef(newMessage);
  newMessageRef.current = newMessage;
  useEffect(() => onTrayAction((a) => a === "compose" && newMessageRef.current()), []);

  // The calendar asked for from elsewhere (an invitation, the assistant's appointment).
  useEffect(() => {
    const go = (e: Event) => {
      const r = (e as CustomEvent<CalendarRequest>).detail;
      goPlace({ kind: "calendar" });
      setCalRequest(r);
    };
    window.addEventListener("arcalo-mail:calendar", go);
    return () => window.removeEventListener("arcalo-mail:calendar", go);
  });

  // A notification's „Öffnen“ or „Antworten“: the message (also when it started the app).
  const openRequested = async () => {
    const r = await api.takePendingOpen().catch(() => null);
    if (!r) return;
    if (placeRef.current.kind === "calendar") goPlace({ kind: "inbox", account: null });
    openById(r.id);
    if (r.reply) {
      const v = await loadView(r.id);
      if (v) replyTo("reply", v);
    }
  };
  const openRequestedRef = useRef(openRequested);
  openRequestedRef.current = openRequested;
  useEffect(() => {
    if (!accounts?.length) return;
    void openRequestedRef.current();
    return onOpenMessage(() => void openRequestedRef.current());
  }, [!!accounts?.length]);

  // E-mail links of other apps (mailto:, src-tauri/src/defaultmail.rs): the compose sheet with
  // what the link names. A link that started the app waits until the accounts are there.
  const mailtoRequested = async () => {
    const m = await api.takePendingMailto().catch(() => null);
    if (m) composeMailto(m);
  };
  /** The compose sheet with what an e-mail link names. */
  const composeMailto = (m: Mailto) => {
    // From the account in view (its inbox or folder, or the one the switcher narrowed to).
    const here = mailbox?.account_id ?? (place.kind === "inbox" ? place.account : null) ?? (results ? searchAccount : null);
    const account = accountById(here) ?? accounts?.[0] ?? null;
    const d = emptyDraft(account, rich, m.to);
    d.cc = m.cc;
    d.bcc = m.bcc;
    d.showCc = m.cc.length > 0;
    d.showBcc = m.bcc.length > 0;
    d.subject = m.subject;
    if (m.body) {
      if (rich) d.html = `${textToEditorHtml(m.body)}${d.html}`;
      else d.text = `${m.body}${d.text}`;
    }
    void startCompose(d);
  };
  const mailtoRequestedRef = useRef(mailtoRequested);
  mailtoRequestedRef.current = mailtoRequested;
  useEffect(() => {
    if (!accounts?.length) return;
    void mailtoRequestedRef.current();
    return onEvent("compose-mailto", () => void mailtoRequestedRef.current());
  }, [!!accounts?.length]);

  // The open message is kept as a draft before the app restarts for an update, before it quits
  // (the shell asks: the window closed, the tray's „Beenden“, Cmd+Q) and when it goes to the
  // background (a phone may end it there).
  const flushRef = useRef(flushDraft);
  flushRef.current = flushDraft;
  useEffect(() => setBeforeRestart(() => flushRef.current()), []);
  useEffect(() => onSaveDrafts(() => flushRef.current()), []);
  useEffect(() => onVisibility((visible) => !visible && void flushRef.current()), []);

  // A moment after the last change the open message is saved.
  useEffect(() => {
    if (!compose || compose.accountId == null || isEmpty(compose) || isSaved(compose)) return;
    const timer = setTimeout(() => void flushRef.current(), AUTOSAVE_MS);
    return () => clearTimeout(timer);
  }, [compose]);

  // Back in front after a while (the window got the focus again): check for mail now.
  useEffect(() => {
    const onFocus = () => {
      if (Date.now() - lastSync.current > 60_000) {
        lastSync.current = Date.now();
        void api.syncNow(null);
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // ---------------------------------------------------------------- settings

  // The settings as last changed here, the saves in order, and which save is the latest: the
  // backend's answer to an older save never undoes a newer change.
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const settingsSaves = useRef<Promise<unknown>>(Promise.resolve());
  const settingsGen = useRef(0);
  const saveSettings = (s: Settings) => {
    settingsRef.current = s;
    setSettings(s);
    const gen = ++settingsGen.current;
    const run = settingsSaves.current.then(async () => {
      await api.saveSettings(s);
      // The backend fills in what follows from a change (when the Screener started).
      const back = await api.settings();
      if (gen === settingsGen.current) setSettings(back);
    });
    settingsSaves.current = run.catch((e) => showToast({ title: t("toast.actionFailed"), detail: String(e), tone: "danger" }));
  };

  const updateSettings = (patch: Partial<Settings>) => {
    const cur = settingsRef.current;
    if (!cur) return;
    // What the Undo puts back: the changed settings only, as they were.
    const before = Object.fromEntries(Object.keys(patch).map((k) => [k, cur[k as keyof Settings]])) as Partial<Settings>;
    saveSettings({ ...cur, ...patch });
    showToast({
      title: t("toast.settingApplied"),
      duration: 4000,
      action: {
        label: t("common.undo"),
        run: () => {
          const latest = settingsRef.current;
          if (latest) saveSettings({ ...latest, ...before });
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
      if (!action || (overlay && action !== "settings" && action !== "palette")) return;
      // Enter on a button, a link or a tab is that control's; the list opens only from the list.
      if (action === "open" && isControl(e.target)) return;
      // The calendar has keys of its own; compose, undo, the settings and the palette stay.
      if (placeRef.current.kind === "calendar" && !showOutbox && !results && !["compose", "undo", "settings", "palette", "search"].includes(action)) return;
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
        case "snooze":
        case "more": {
          e.preventDefault();
          const button = document.querySelector<HTMLElement>(`.reading-pane .toolbar [data-action="${action}"]`);
          act(action, button?.getBoundingClientRect());
          break;
        }
        case "pin": {
          const ids = targets();
          const latest = loadedViews()[0];
          if (ids.length && latest) void setPinned(ids, !latest.marks.pinned);
          break;
        }
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
        case "palette":
          e.preventDefault();
          setPalette((p) => !p && !document.querySelector(".overlay:not(.palette-overlay)"));
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // ---------------------------------------------------------------- places

  const goPlace = (p: Place, mailbox: number | null = null) => {
    // The view the user went to, for the next start on this device (where the app placed the
    // list by itself, as with the first account, it decides again).
    if (p.kind === "inbox" || p.kind === "role" || (p.kind === "folder" && mailbox != null)) rememberView(p, mailbox);
    // An inbox opens at its first tab.
    if (p.kind === "inbox") setTab("people");
    setQuery("");
    setShowOutbox(false);
    setMailboxId(mailbox);
    setPlace(p);
    setView("list");
  };

  /** A folder of the sidebar; an inbox opens sorted. */
  const goMailbox = (m: Mailbox) => goPlace(m.role === "inbox" ? { kind: "inbox", account: m.account_id } : { kind: "folder" }, m.id);

  // ---------------------------------------------------------------- switching accounts

  /** The folder the switcher moves between accounts: the inbox, or Drafts, Sent or Archive. */
  const switchRole: "inbox" | UnifiedRole | null =
    place.kind === "inbox"
      ? "inbox"
      : place.kind === "role"
        ? place.role
        : place.kind === "folder" && (mailbox?.role === "inbox" || mailbox?.role === "drafts" || mailbox?.role === "sent" || mailbox?.role === "archive")
          ? mailbox.role
          : null;
  const switchCurrent = results ? searchAccount : place.kind === "inbox" ? place.account : place.kind === "role" ? null : (mailbox?.account_id ?? null);
  const inboxUnread = (id: number) => acctCounts[id] ?? (mailboxes[id] ?? []).filter((m) => m.role === "inbox").reduce((n, m) => n + m.unread, 0);
  const pickAccount = (id: number | null) => {
    if (results) return setSearchAccount(id);
    if (!switchRole) return;
    if (id == null) return goPlace(switchRole === "inbox" ? { kind: "inbox", account: null } : { kind: "role", role: switchRole });
    const box = (mailboxes[id] ?? []).find((m) => m.role === switchRole) ?? (mailboxes[id] ?? []).find((m) => m.role === "inbox");
    if (box) goMailbox(box);
  };

  /** The color (1–6) of an account's dot, with two accounts or more. */
  const accountColor = (id: number): number | null => {
    if (!multi) return null;
    const i = accounts?.findIndex((a) => a.id === id) ?? -1;
    return i < 0 ? null : (i % 6) + 1;
  };

  /** The entries above the accounts (`all`: also the empty ones, for the palette). */
  const viewEntries = (all = false): ViewEntry[] => {
    if (!settings) return [];
    const out: ViewEntry[] = [];
    const here = (k: Place["kind"]) => !showOutbox && place.kind === k;
    const add = (id: Exclude<Place["kind"], "folder" | "inbox" | "role">, label: string, icon: ViewEntry["icon"], count: number, show: boolean) => {
      if (show || here(id) || all) out.push({ id, label, icon, count, current: here(id), onSelect: () => goPlace({ kind: id }) });
    };
    if (multi && settings.unified_inbox) {
      const unread = Object.values(mailboxes)
        .flat()
        .filter((m) => m.role === "inbox")
        .reduce((n, m) => n + m.unread, 0);
      out.push({
        id: "unified",
        label: t("inbox.unified"),
        icon: Layers,
        count: unread,
        current: !showOutbox && place.kind === "inbox" && place.account == null,
        onSelect: () => goPlace({ kind: "inbox", account: null }),
      });
      const ICON: Record<UnifiedRole, ViewEntry["icon"]> = { drafts: FileText, sent: Send, archive: Archive };
      for (const role of UNIFIED_ROLES) {
        if (!Object.values(mailboxes).some((list) => list.some((m) => m.role === role))) continue;
        out.push({
          id: `unified-${role}`,
          label: t(`unified.${role}`),
          icon: ICON[role],
          count: 0,
          current: !showOutbox && place.kind === "role" && place.role === role,
          onSelect: () => goPlace({ kind: "role", role }),
        });
      }
    }
    add("screener", t("folder.screener"), UserPlus, screenerCount, settings.screener);
    add("snoozed", t("folder.snoozed"), Clock, times.snoozed, times.snoozed > 0);
    add("later", t("folder.later"), BookmarkPlus, times.later, times.later > 0);
    add("followups", t("folder.followups"), BellRing, times.followups, times.followups > 0);
    add("scheduled", t("folder.scheduled"), Send, times.scheduled, times.scheduled > 0);
    add("subscriptions", t("folder.subscriptions"), Newspaper, 0, false);
    add("calendar", t("nav.calendar"), CalendarDays, 0, true);
    add("files", t("folder.files"), Paperclip, 0, true);
    add("ask", t("folder.ask"), MessagesSquare, 0, aiOn);
    return out;
  };

  // ---------------------------------------------------------------- the command palette

  const paletteCommands = (): Command[] => {
    const out: Command[] = [
      { id: "compose", label: t("cmd.compose"), hint: "c", icon: PenSquare, run: () => newMessage() },
      { id: "sync", label: t("cmd.sync"), icon: RefreshCw, run: () => void api.syncNow(null) },
    ];
    if (outbox.length) out.push({ id: "outbox", label: t("cmd.outbox"), icon: Upload, run: () => setShowOutbox(true) });
    for (const v of viewEntries(true)) out.push({ id: `view-${v.id}`, label: t("cmd.goTo", { name: v.label }), icon: v.icon, run: v.onSelect });
    out.push({ id: "focus", label: t("cmd.focus"), icon: Crosshair, run: () => updateSettings({ focus: !focus }) });
    for (const a of accounts ?? []) {
      for (const m of mailboxes[a.id] ?? []) {
        if (!listedFolder(m)) continue;
        const name = folderLabel(m, t);
        out.push({
          id: `folder-${m.id}`,
          label: t("cmd.goTo", { name }),
          keywords: `${m.name} ${a.name} ${a.email}`,
          hint: (accounts?.length ?? 0) > 1 ? a.name || a.email : undefined,
          icon: m.role === "inbox" ? Inbox : folderIcon(m),
          run: () => goMailbox(m),
        });
      }
    }
    const sections: [SettingsSection, Parameters<typeof t>[0]][] = [
      ["accounts", "settings.accounts"],
      ["appearance", "settings.appearance"],
      ["reading", "settings.reading"],
      ["inbox", "settings.inbox"],
      ["writing", "settings.writing"],
      ["templates", "settings.templates"],
      ["rules", "settings.rules"],
      ["contacts", "settings.contacts"],
      ["ai", "settings.ai"],
      ["notifications", "settings.notifications"],
      ["privacy", "settings.privacy"],
      ["network", "settings.network"],
      ["shortcuts", "settings.shortcuts"],
      ["about", "settings.about"],
    ];
    out.push({ id: "add-account", label: t("cmd.addAccount"), icon: CircleUserRound, run: () => setSettingsOpen("accounts") });
    for (const [id, key] of sections) {
      out.push({ id: `settings-${id}`, label: t("cmd.settings", { name: t(key) }), keywords: id, icon: SettingsIcon, run: () => setSettingsOpen(id) });
    }
    const themes: [ThemeChoice, Parameters<typeof t>[0]][] = [
      ["system", "theme.system"],
      ["light", "theme.light"],
      ["dark", "theme.dark"],
      ["contrast", "theme.contrast"],
    ];
    for (const [theme, key] of themes) {
      out.push({ id: `theme-${theme}`, label: t("cmd.theme", { name: t(key) }), keywords: theme, icon: PaletteIcon, run: () => updateSettings({ theme }) });
    }
    out.push({ id: "diagnostics", label: t("cmd.diagnostics"), icon: Download, run: () => void exportDiagnostics(t) });
    return out;
  };

  const searchCommand = (q: string): Command => ({
    id: "search",
    label: t("cmd.search", { q }),
    hint: "/",
    icon: Search,
    run: () => {
      setShowOutbox(false);
      setQuery(q);
    },
  });

  // A phone opens the settings at their list of sections unless a section was asked for.
  useEffect(() => {
    if (!settingsOpen) setSettingsNav(true);
  }, [settingsOpen]);

  // ---------------------------------------------------------------- the system's Back

  // Android's Back button and gesture walk the app's screens: each screen with a way back keeps
  // one entry in the WebView's history, and Back (popstate) takes that way.
  const phoneSettings = !!settingsOpen && layout === "phone";
  const step = backStep({ view, layout, composing: !!compose, settings: !!settingsOpen });
  const stepRef = useRef(step);
  stepRef.current = step;
  // Our own history.back() (the last screen closed) is not a Back of the user: its popstate
  // only brings the entries in line with the screen shown by then (another one may have opened
  // meanwhile).
  const ownBacks = useRef(0);
  const syncBack = useRef(() => {});
  syncBack.current = () => {
    if (!touch) return;
    const marked = (window.history.state as { arcaloBack?: boolean } | null)?.arcaloBack === true;
    const way = stepRef.current || openLayers() > 0;
    if (way && !marked) window.history.pushState({ arcaloBack: true }, "");
    else if (!way && marked) {
      ownBacks.current++;
      window.history.back();
    }
  };
  const goBack = useRef<() => void>(() => {});
  goBack.current = () => {
    if (step === "settings" && phoneSettings && !settingsNav) return setSettingsNav(true);
    // The sheet stays when its draft cannot be saved: it keeps its way back then.
    if (step === "compose") return void closeCompose().then(() => syncBack.current());
    if (step === "settings") return setSettingsOpen(null);
    if (step === "list") return setView("list");
  };
  useEffect(() => {
    if (!touch) return;
    const onPop = () => {
      if (ownBacks.current > 0) {
        ownBacks.current--;
        syncBack.current();
        return;
      }
      // A menu, sheet or dialog over the screen closes first.
      if (closeTopLayer()) return;
      goBack.current();
    };
    const onLayers = () => syncBack.current();
    window.addEventListener("popstate", onPop);
    window.addEventListener(LAYERS_EVENT, onLayers);
    return () => {
      window.removeEventListener("popstate", onPop);
      window.removeEventListener(LAYERS_EVENT, onLayers);
    };
  }, [touch]);
  // Before the screen is painted: a Back right after a screen showed finds its entry (after
  // the paint it came too late, and Back at the bottom of the history leaves the app).
  useLayoutEffect(() => syncBack.current(), [touch, step, settingsNav]);

  // ---------------------------------------------------------------- swipes and pull to refresh

  /** The messages a row stands for: the conversation in this folder, or the one message. */
  const rowIds = async (m: MessageSummary): Promise<number[]> => {
    if ((m.bundle ?? 0) > 0 && m.from && place.kind === "inbox") return (await api.bundle(place.account, m.from.email)).map((x) => x.id);
    if (!threads || m.thread_count <= 1) return [m.id];
    const all = await api.thread(m.thread_id);
    const here = all.filter((x) => x.mailbox_id === m.mailbox_id).map((x) => x.id);
    return here.length ? here : [m.id];
  };

  const swiped = async (m: MessageSummary, a: SwipeAction) => {
    const ids = await rowIds(m);
    switch (a) {
      case "archive":
        return withUndo(t("toast.archived", { n: 1 }), ids, () => api.archive(ids));
      case "delete":
        return withUndo(t("toast.deleted", { n: 1 }), ids, () => api.remove(ids));
      case "read":
        patchRows([m.id], { seen: !m.seen });
        return setFlags(ids, !m.seen, null);
      case "flag":
        patchRows([m.id], { flagged: !m.flagged });
        return setFlags(ids, null, !m.flagged);
    }
  };

  const refresh = async () => {
    lastSync.current = Date.now();
    await api.syncNow(mailbox?.account_id ?? null);
    // The check runs in the background; the spinner stays a moment so the pull is answered.
    await new Promise((r) => setTimeout(r, 1200));
  };

  // ---------------------------------------------------------------- render

  if (!settings || !accounts || !info) return <div className="app app-loading" />;

  if (accounts.length === 0)
    return (
      <div className="app app-setup">
        {!inApp() && <div className="demo-bar">{t("app.demo")}</div>}
        <Setup
          welcome
          onDone={() =>
            void Promise.all([loadAccounts(), api.settings()]).then(([{ list, all }, s]) => {
              setSettings(s);
              openStart(list, all, s.unified_inbox);
            })
          }
        />
        <UpdateLayer />
        <Toasts />
      </div>
    );

  const placeTitle = (): string => {
    switch (place.kind) {
      case "inbox":
        return place.account == null ? t("inbox.unified") : mailbox ? folderLabel(mailbox, t) : t("folder.inbox");
      case "snoozed":
        return t("folder.snoozed");
      case "later":
        return t("folder.later");
      case "followups":
        return t("folder.followups");
      case "role":
        return t(`unified.${place.role}`);
      default:
        return mailbox ? folderLabel(mailbox, t) : t("app.name");
    }
  };
  const title = results ? t("list.searchLabel") : placeTitle();
  const emptyText = results
    ? t("list.noResults", { q: query.trim() })
    : place.kind === "snoozed"
      ? t("snoozed.empty")
      : place.kind === "later"
        ? t("later.empty")
        : place.kind === "followups"
          ? t("followups.empty")
          : mailbox || place.kind === "inbox"
            ? t("list.empty")
            : t("list.syncing");
  const timeList = !results && (place.kind === "snoozed" || place.kind === "later" || place.kind === "followups");
  const placeNote = place.kind === "snoozed" ? t("snoozed.note") : place.kind === "later" ? t("later.note") : place.kind === "followups" ? t("followups.note") : null;
  const inInbox = !results && place.kind === "inbox";
  const mobile = info.platform === "android" || info.platform === "ios";
  const dueRow = current != null && place.kind === "followups" ? rowAt(order.indexOf(current)) : undefined;
  const openMessages = (m: MessageSummary, byId = false) => {
    setSel({ ids: [m.id], anchor: m.id });
    void open(m, byId);
  };

  /** Opens a message known only by its id (a notification, an answer's source, a file of the
   *  hub, a subscription): its own row first, so it is read and shown with its conversation like
   *  a message opened in the list. */
  const openById = (id: number) => {
    const seq = ++openByIdSeq.current;
    void api
      .message(id)
      .then((v) => {
        if (seq !== openByIdSeq.current) return;
        openMessages({ ...v.summary, account_id: v.account_id }, true);
      })
      .catch((e) => showToast({ title: t("read.loadFailed"), detail: String(e), tone: "danger" }));
  };

  /** The assistant's reply draft in a reply to the newest message of the conversation. */
  const replyWith = (text: string) => {
    const v = latestView();
    if (!v) return;
    const d = replyDraft(v, "reply", accountById(v.account_id), currentLang(), rich);
    void startCompose(replaceOwnText(d, text));
  };

  const hubAttachment = (it: { id: number; message_id: number; filename: string | null; mime_type: string; size: number }, how: "open" | "save" | "preview") =>
    attachment({ id: it.id, message_id: it.message_id, part_index: 0, filename: it.filename, mime_type: it.mime_type, size: it.size, content_id: null, inline: false, section: null }, how);

  return (
    <div className="app" data-view={view} data-layout={layout} data-short={short ? "" : undefined} data-touch={touch ? "" : undefined} data-composing={compose ? "" : undefined}>
      {!inApp() && <div className="demo-bar">{t("app.demo")}</div>}
      <div className="panes">
        {layout === "tablet" && view === "folders" && <div className="drawer-scrim" onClick={() => setView("list")} aria-hidden />}
        <Sidebar
          accounts={accounts}
          mailboxes={mailboxes}
          current={place.kind === "folder" || (place.kind === "inbox" && place.account != null) ? mailboxId : null}
          showingOutbox={showOutbox}
          outboxCount={outbox.length}
          outboxWaiting={outbox.some((e) => e.state === "failed" || e.state === "parked")}
          views={viewEntries()}
          accountColor={accountColor}
          onSelect={goMailbox}
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
        ) : !results && place.kind === "screener" ? (
          <ScreenerView
            senders={waiting}
            current={current}
            onOpen={openMessages}
            onAllow={(w) =>
              void api.screenerAllow(w.email).then(() => {
                reloadSmart();
                refreshList();
                showToast({ title: t("screener.allowed", { name: w.name || w.email }), tone: "success", duration: 3000 });
              })
            }
            onBlock={(w) =>
              void api.screenerBlock(w.email).then((token) => {
                reloadCounts();
                if (w.messages.some((m) => m.id === current)) {
                  setCurrent(null);
                  setThread([]);
                }
                const back = async () => {
                  await api.undo(token);
                  await api.screenerForget(w.email);
                  reloadCounts();
                  showToast({ title: t("toast.undone"), duration: 2500 });
                };
                setUndo(() => void back());
                showToast({ title: t("screener.blocked", { name: w.name || w.email }), duration: 5000, action: { label: t("common.undo"), run: () => void back() } });
              })
            }
            onShowFolders={() => setView("folders")}
          />
        ) : !results && place.kind === "subscriptions" ? (
          <SubscriptionsView
            list={subs}
            busy={unsubBusy}
            onOpen={(s) => openById(s.message_id)}
            onUnsubscribe={(s) => void unsubscribe(s.message_id, s.name || s.sender)}
            onShowFolders={() => setView("folders")}
          />
        ) : !results && place.kind === "calendar" ? (
          <CalendarView
            settings={settings}
            onSettings={() => setSettingsOpen("calendar")}
            onShowFolders={() => setView("folders")}
            request={calRequest}
            onRequestTaken={() => setCalRequest(null)}
            phone={layout === "phone"}
          />
        ) : !results && place.kind === "ask" ? (
          <AskView on={aiOn} onOpen={(s) => openById(s.message_id)} onShowFolders={() => setView("folders")} onSettings={() => setSettingsOpen("ai")} />
        ) : !results && place.kind === "files" ? (
          <AttachmentHub
            accountId={null}
            revision={searchRev}
            onPreview={(it) => void hubAttachment(it, "preview")}
            onSave={(it) => void hubAttachment(it, "save")}
            onOpenFile={(it) => void hubAttachment(it, "open")}
            onOpenMessage={(it) => openById(it.message_id)}
            onShowFolders={() => setView("folders")}
          />
        ) : !results && place.kind === "scheduled" ? (
          <ScheduledView
            list={scheduled}
            accounts={accounts}
            mobile={mobile}
            onSendNow={(s) =>
              void api.sendScheduledNow(s.id).then(() => {
                reloadSmart();
                void api.outbox().then(setOutbox);
                showToast({ title: t("toast.queued"), duration: 3000 });
              })
            }
            onEdit={(s) =>
              void api.unschedule(s.id).then((d) => {
                reloadSmart();
                void editStored(d);
              })
            }
            onDiscard={(s) =>
              void api.unschedule(s.id).then((d) => {
                reloadSmart();
                showToast({
                  title: t("toast.draftDiscarded"),
                  duration: 5000,
                  action: s.due_at > clockNow().getTime() / 1000 ? { label: t("common.undo"), run: () => void api.sendLater(d, s.due_at, s.followup_secs).then(reloadSmart) } : undefined,
                });
              })
            }
            onShowFolders={() => setView("folders")}
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
            avatars={layout === "phone"}
            loading={!results && list.loading}
            emptyText={emptyText}
            onSwipeAccount={
              multi && (switchRole || results) && layout === "phone"
                ? (dir) => pickAccount(stepAccount((accounts ?? []).map((a) => a.id), switchCurrent, dir))
                : undefined
            }
            head={
              <>
                {multi && (switchRole || results) && (
                  <AccountSwitcher
                    accounts={switcherAccounts(accounts, inboxUnread, accountColor)}
                    current={switchCurrent}
                    total={(accounts ?? []).reduce((n, a) => n + inboxUnread(a.id), 0)}
                    onPick={pickAccount}
                  />
                )}
                {inInbox ? (
                  <InboxHead
                    tab={tab}
                    counts={counts}
                    focus={focus}
                    onTab={setTab}
                    onFocusEnd={() => updateSettings({ focus: false })}
                    onFocus={layout === "phone" ? () => updateSettings({ focus: true }) : undefined}
                  />
                ) : timeList && placeNote ? (
                  <p className="muted smart-note">{placeNote}</p>
                ) : undefined}
              </>
            }
            titleExtra={
              inInbox ? (
                <span className="list-title-tools">
                  {tab === "newsletters" && !focus && (
                    <button type="button" className="btn btn-sm btn-quiet" onClick={() => goPlace({ kind: "subscriptions" })}>
                      <Newspaper size={14} strokeWidth={1.75} aria-hidden />
                      <span>{t("inbox.subscriptions")}</span>
                    </button>
                  )}
                  <button
                    type="button"
                    className="icon-btn focus-btn"
                    aria-pressed={focus}
                    onClick={() => updateSettings({ focus: !focus })}
                    aria-label={t("inbox.focusToggle")}
                    title={t("inbox.focusToggle")}
                  >
                    <Crosshair size={16} strokeWidth={1.75} aria-hidden />
                  </button>
                </span>
              ) : undefined
            }
            empty={inInbox ? <InboxZero tab={tab} focus={focus} counts={counts} snoozed={times.snoozed} onTab={setTab} /> : undefined}
            accountOf={(m) => {
              const c = multi && (results || place.kind !== "inbox" || place.account == null) ? accountColor(m.account_id) : null;
              const a = c != null ? accounts.find((x) => x.id === m.account_id) : undefined;
              return c != null && a ? { color: c, name: a.name || a.email } : null;
            }}
            dueOf={(m) => (!results && (place.kind === "snoozed" || place.kind === "followups") && m.due ? whenLabel(m.due, clockNow(), currentLang()) : null)}
            onQuery={setQuery}
            onServerSearch={() => void serverSearch()}
            onRowClick={(i, mods) => {
              // A Shift range over pages never loaded: they are loaded first, so it has no holes.
              const gaps = mods.shift && !results ? rangeGaps(sel, order, i, PAGE) : [];
              if (gaps.length) {
                void loadPages(gaps).then((loaded) => {
                  const full = order.map((id, j) => id ?? loaded[Math.floor(j / PAGE)]?.[j % PAGE]?.id);
                  setSel((s) => clickSelect(s, full, i, mods));
                });
                listRef.current?.focus();
                return;
              }
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
            touch={touch ? { left: settings.swipe_left, right: settings.swipe_right, onSwipe: (m, a) => void swiped(m, a), onRefresh: refresh } : undefined}
            extra={
              touch && layout !== "desktop" && !results ? (
                <button type="button" className="compose-fab" onClick={() => newMessage()} aria-label={t("nav.compose")}>
                  <PenSquare size={20} strokeWidth={1.75} aria-hidden />
                  <span>{t("nav.compose")}</span>
                </button>
              ) : undefined
            }
          />
        )}
        {(showOutbox || results || place.kind !== "calendar") && <ReadingPane
          onOpenCalendar={(key, start) => openCalendar({ key, day: start })}
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
          onEditDraft={(v) =>
            void api
              .draftOf(v.summary.id)
              .then(editStored)
              .catch((e) => showToast({ title: t("read.loadFailed"), detail: String(e), tone: "danger" }))
          }
          touch={touch}
          inbox={inInbox}
          stream={stream ? { name: stream.name } : null}
          banner={
            dueRow?.due ? (
              <div className="notice reminder-banner" role="status">
                <BellRing size={16} strokeWidth={1.75} aria-hidden />
                <span>{t("read.noAnswer", { when: whenLabel(dueRow.due, clockNow(), currentLang()) })}</span>
                <button type="button" className="btn btn-sm" onClick={() => void dismissFollowups(targets())}>
                  {t("followup.dismiss")}
                </button>
              </div>
            ) : undefined
          }
          onUnsubscribe={(v) => v.summary.from && void unsubscribe(v.summary.id, displayName(v.summary.from, currentLang()))}
          onMark={onMark}
          onContact={(a) => setContact({ email: a.email, name: a.name })}
          assistant={
            thread.length && aiOn && !stream ? (
              <AssistantBar
                key={`${thread[thread.length - 1].id}-${current ?? 0}`}
                messageId={current ?? thread[thread.length - 1].id}
                latestId={thread[thread.length - 1].id}
                accountId={(thread.find((m) => m.id === current) ?? thread[thread.length - 1]).account_id || (latestView()?.account_id ?? 0)}
                on={aiOn}
                autoSummary={settings.ai.auto_summary}
                onDraft={replyWith}
                onSettings={() => setSettingsOpen("ai")}
              />
            ) : undefined
          }
        />}
      </div>
      {compose && (
        <ComposeSheet
          key={compose.uid}
          draft={compose}
          accounts={accounts}
          error={composeError}
          sending={sending}
          saved={isSaved(compose)}
          modal={fullScreen}
          phone={fullScreen}
          onChange={(d) => setCompose((c) => (c && c.uid === d.uid ? d : c))}
          onSend={(followupSecs) => void send(followupSecs)}
          onSendLater={(at, followupSecs) => (at == null ? setWhen({ kind: "send", followupSecs }) : void sendLater(at, followupSecs))}
          onSaveDraft={(d) => void saveDraft(committed(d), false)}
          onClose={() => void closeCompose()}
          onDiscard={discardCompose}
          aiOn={aiOn}
          signByDefault={settings.sign_by_default}
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
          touch={touch}
          phone={layout === "phone" ? { nav: settingsNav, onNav: setSettingsNav } : undefined}
          onAccountsChanged={() =>
            void loadAccounts().then(({ list, all }) => {
              if (mailboxId != null && !Object.values(all).flat().some((b) => b.id === mailboxId)) openStart(list, all, settings.unified_inbox);
              else if (place.kind === "inbox" && place.account == null && list.length < 2) openStart(list, all, false);
            })
          }
          onClose={() => setSettingsOpen(null)}
          onSubscriptions={() => goPlace({ kind: "subscriptions" })}
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
              const inbox = mailboxes[m.account_id]?.find((b) => b.role === "inbox");
              if (mailboxId === m.id && inbox) goMailbox(inbox);
            });
          }}
          onClose={() => setFolderDialog(null)}
        />
      )}
      {signIn &&
        (signIn.transport.kind === "imap" && signIn.transport.auth.kind === "o_auth2" ? (
          <MicrosoftSignInDialog account={signIn} onClose={() => setSignIn(null)} />
        ) : (
          <SignInDialog account={signIn} onClose={() => setSignIn(null)} />
        ))}
      {palette && <CommandPalette commands={paletteCommands()} search={searchCommand} onClose={() => setPalette(false)} />}
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("read.actions")} />}
      {when && (
        <WhenDialog
          title={when.kind === "snooze" ? t("snooze.title") : when.kind === "send" ? t("compose.sendLater") : t("read.followup")}
          confirm={when.kind === "snooze" ? t("read.snooze") : when.kind === "send" ? t("compose.schedule") : t("when.remind")}
          note={when.kind === "send" ? (mobile ? t("scheduled.noteMobile") : t("scheduled.note")) : undefined}
          onPick={(at) => {
            if (when.kind === "snooze") void snooze(when.ids, at);
            else if (when.kind === "followup") void setFollowup(when.ids, at);
            else void sendLater(at, when.followupSecs);
          }}
          onClose={() => setWhen(null)}
        />
      )}
      {takeBack && (
        <Dialog
          title={t("compose.takeBackTitle")}
          above
          onClose={() => {
            // Not opened: the message waits in Drafts.
            const back = takeBack;
            setTakeBack(null);
            void saveDraft(back, true).then((ok) => showToast(ok ? { title: t("compose.takeBackFiled"), duration: 4000 } : { title: t("toast.draftFailed"), tone: "danger" }));
          }}
          footer={
            <>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  const back = takeBack;
                  setTakeBack(null);
                  void saveDraft(back, true).then((ok) => showToast(ok ? { title: t("compose.takeBackFiled"), duration: 4000 } : { title: t("toast.draftFailed"), tone: "danger" }));
                }}
              >
                {t("compose.takeBackKeep")}
              </button>
              <button
                type="button"
                className="btn btn-primary"
                autoFocus
                onClick={() => {
                  const back = takeBack;
                  setTakeBack(null);
                  void startCompose(back);
                }}
              >
                {t("compose.takeBackOpen")}
              </button>
            </>
          }
        >
          <p className="dialog-text">{t("compose.takeBackText", { subject: takeBack.subject.trim() || t("list.noSubject") })}</p>
        </Dialog>
      )}
      {confirm && <ConfirmDialog title={confirm.title} text={confirm.text} detail={confirm.detail} confirm={confirm.confirm} onConfirm={confirm.run} onClose={() => setConfirm(null)} />}
      {sendCheck?.kind === "unlock" && <PassphraseDialog above fingerprint={sendCheck.fingerprint} onDone={sendCheck.go} onClose={() => setSendCheck(null)} />}
      {sendCheck?.kind === "plain" && (
        <ConfirmDialog
          title={t("protect.sendUnencryptedTitle")}
          text={t("protect.quotesEncrypted")}
          confirm={t("protect.sendUnencrypted")}
          danger
          above
          onConfirm={sendCheck.go}
          onClose={() => setSendCheck(null)}
        />
      )}
      {risky && (
        <Dialog
          title={risky.risk === "blocked" ? t("read.blockedTitle") : t("read.macroTitle")}
          onClose={() => setRisky(null)}
          footer={
            <>
              <button type="button" className="btn btn-quiet" onClick={() => setRisky(null)}>
                {t("common.cancel")}
              </button>
              {risky.risk === "macro" && (
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    const a = risky.a;
                    setRisky(null);
                    void attachment(a, "open_anyway");
                  }}
                >
                  {t("read.openAnyway")}
                </button>
              )}
              <button
                type="button"
                className="btn btn-primary"
                autoFocus
                onClick={() => {
                  const a = risky.a;
                  setRisky(null);
                  void attachment(a, "save");
                }}
              >
                {t("read.save")}
              </button>
            </>
          }
        >
          <p className="dialog-text">{t(risky.risk === "blocked" ? "read.blockedText" : "read.macroText", { name: risky.a.filename ?? t("read.unnamed") })}</p>
        </Dialog>
      )}
      {preview && (
        <div className="overlay preview-overlay" onMouseDown={(e) => e.target === e.currentTarget && setPreview(null)} onKeyDown={(e) => e.key === "Escape" && setPreview(null)}>
          <figure className="preview" role="dialog" aria-modal="true" aria-label={preview.name}>
            <figcaption>
              <span>{preview.name}</span>
              <button type="button" className="icon-btn" autoFocus onClick={() => setPreview(null)} aria-label={t("common.close")}>
                <X size={16} strokeWidth={1.75} aria-hidden />
              </button>
            </figcaption>
            {preview.text != null ? <pre className="preview-text">{preview.text}</pre> : <img src={preview.url} alt={preview.name} />}
          </figure>
        </div>
      )}
      {contact && (
        <ContactCardDialog
          email={contact.email}
          name={contact.name}
          onClose={() => setContact(null)}
          onCompose={(to) => newMessage([to])}
          onOpen={(m) => ("subject" in m ? openMessages(m as MessageSummary) : openById(m.id))}
          onSave={(f) => void hubAttachment(f, "save")}
        />
      )}
      <UpdateLayer />
      <Toasts />
    </div>
  );
}

/** A Microsoft account whose sign-in ended: sign in again in the browser. */
function MicrosoftSignInDialog({ account, onClose }: { account: AccountInfo; onClose: () => void }) {
  const t = useT();
  const [state, setState] = useState<{ busy: boolean; result: MicrosoftSignIn | null }>({ busy: false, result: null });
  const problem = state.result?.problem ?? account.status?.problem ?? null;
  const go = async () => {
    setState({ busy: true, result: null });
    try {
      const result = await api.microsoftReauth(account.id);
      if (!result.problem && !result.error) onClose();
      else setState({ busy: false, result });
    } catch (e) {
      setState({ busy: false, result: { session: null, email: null, check: null, problem: null, error: String(e) } });
    }
  };
  return (
    <Dialog
      title={t("signIn.title", { email: account.email })}
      description={t("signIn.microsoftText")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={state.busy} onClick={() => void go()}>
            <LogIn size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("signIn.browser")}</span>
          </button>
        </>
      }
    >
      {state.busy && <p className="muted">{t("ms.waiting")}</p>}
      {problem && <ProblemCard which="microsoft" p={problem} />}
      {state.result?.error && (
        <p className="field-error" role="alert">
          {state.result.error}
        </p>
      )}
    </Dialog>
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
