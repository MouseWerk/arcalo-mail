// The backend: the Tauri commands (src-tauri/src/commands.rs), or the sample data when the UI
// runs in a plain browser (`npm run dev`, lib/demo.ts). The UI never talks to the network or
// the file system itself.

import { invoke } from "@tauri-apps/api/core";
import { demo } from "./demo";
import type {
  AccountDraft,
  AccountInfo,
  AppInfo,
  CheckResult,
  ComposeDraft,
  Contact,
  Discovered,
  FileInfo,
  Mailbox,
  MessageSummary,
  MessageView,
  MicrosoftSignIn,
  Notice,
  OutboxEntry,
  SystemProxy,
  Page,
  Queued,
  Settings,
  Transport,
  Undo,
  Account,
  Category,
  InboxCounts,
  InboxQuery,
  Reminder,
  Scheduled,
  Subscription,
  TimeCounts,
  Unsubscribed,
  WaitingSender,
  AddressBook,
  AiOutcome,
  AiProvider,
  AiRequest,
  ContactCard,
  Destination,
  HubFilter,
  HubItem,
  HubSender,
  Rule,
  RuleReport,
  RuleServer,
  Server,
  Template,
  Tone,
  AiItem,
  Pop3Transport,
} from "./types";

export const inApp = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(cmd: string, args: Record<string, unknown>, fallback: () => T | Promise<T>): Promise<T> {
  return inApp() ? invoke<T>(cmd, args) : Promise.resolve().then(fallback);
}

export const api = {
  appInfo: () => call<AppInfo>("app_info", {}, () => demo.appInfo()),
  settings: () => call<Settings>("get_settings", {}, () => demo.settings()),
  saveSettings: (settings: Settings) => call<void>("set_settings", { settings }, () => demo.saveSettings(settings)),

  discover: (email: string) => call<Discovered>("discover", { email }, () => demo.discover(email)),
  check: (transport: Transport, password: string) => call<CheckResult>("check_account", { transport, password }, () => demo.check(password)),
  addAccount: (draft: AccountDraft, password: string) => call<Account>("add_account", { draft, password }, () => demo.addAccount(draft)),
  updateAccount: (id: number, name: string, senderName: string, signature: string) =>
    call<void>("update_account", { id, name, senderName, signature }, () => demo.updateAccount(id, name, senderName, signature)),
  updateServers: (id: number, transport: Transport, password: string | null) =>
    call<void>("update_servers", { id, transport, password }, () => undefined),
  removeAccount: (id: number) => call<void>("remove_account", { id }, () => demo.removeAccount(id)),
  signIn: (id: number, password: string) => call<void>("sign_in", { id, password }, () => undefined),
  microsoftSignIn: (email: string) => call<MicrosoftSignIn>("microsoft_sign_in", { email }, () => demo.microsoftSignIn(email)),
  addMicrosoftAccount: (session: string, name: string) =>
    call<Account>("add_microsoft_account", { session, name }, () => demo.addMicrosoftAccount(session, name)),
  microsoftReauth: (id: number) => call<MicrosoftSignIn>("microsoft_reauth", { id }, () => demo.microsoftSignIn("")),
  /** The POP3 settings of an address (a preset or a guess). */
  pop3Preset: (email: string) =>
    call<Pop3Transport | null>("pop3_preset", { email }, () => ({
      kind: "pop3",
      pop3: { host: `pop.${email.split("@")[1] ?? "example.org"}`, port: 995, security: "tls" },
      smtp: { host: `smtp.${email.split("@")[1] ?? "example.org"}`, port: 465, security: "tls" },
      username: email,
      auth: { kind: "password" },
      leave_on_server: true,
      delete_after_days: null,
    })),
  checkJmap: (sessionUrl: string, username: string, password: string, token: boolean) =>
    call<CheckResult>("check_jmap", { sessionUrl, username, password, token }, () => demo.check(password)),
  setProxyPassword: (profile: string, password: string) => call<void>("set_proxy_password", { profile, password }, () => undefined),
  systemProxy: () => call<SystemProxy>("system_proxy", {}, () => ({ http: null, https: null, socks: null, bypass: "", source: "browser" })),
  exportDiagnostics: (path: string) => call<void>("export_diagnostics", { path }, () => undefined),
  accounts: () => call<AccountInfo[]>("list_accounts", {}, () => demo.accounts()),
  syncNow: (accountId: number | null) => call<void>("sync_now", { accountId }, () => undefined),
  /** The app is visible or hidden (a phone keeps its connections only while it is in front). */
  setInFront: (visible: boolean) => call<void>("set_in_front", { visible }, () => undefined),

  mailboxes: (accountId: number) => call<Mailbox[]>("list_mailboxes", { accountId }, () => demo.mailboxes(accountId)),
  messages: (mailboxId: number, offset: number, limit: number, threads: boolean) =>
    call<Page>("list_messages", { mailboxId, offset, limit, threads }, () => demo.messages(mailboxId, offset, limit, threads)),
  thread: (threadId: number) => call<MessageSummary[]>("list_thread", { threadId }, () => demo.thread(threadId)),
  message: (id: number) => call<MessageView>("get_message", { id }, () => demo.message(id)),
  allowRemote: (id: number) => call<void>("allow_remote_content", { id }, () => demo.allowRemote(id)),
  allowSender: (email: string) => call<void>("allow_sender", { email }, () => demo.allowSender(email)),
  forbidSender: (email: string) => call<void>("forbid_sender", { email }, () => demo.forbidSender(email)),
  allowedSenders: () => call<string[]>("allowed_senders", {}, () => demo.allowedSenders()),

  setFlags: (ids: number[], seen: boolean | null, flagged: boolean | null) =>
    call<void>("set_flags", { ids, seen, flagged }, () => demo.setFlags(ids, seen, flagged)),
  move: (ids: number[], mailboxId: number) => call<Undo>("move_messages", { ids, mailboxId }, () => demo.hide(ids)),
  archive: (ids: number[]) => call<Undo>("archive_messages", { ids }, () => demo.hide(ids)),
  remove: (ids: number[]) => call<Undo>("delete_messages", { ids }, () => demo.hide(ids)),
  spam: (ids: number[]) => call<Undo>("mark_spam", { ids }, () => demo.hide(ids)),
  undo: (token: Undo) => call<boolean>("undo", { token }, () => demo.unhide(token)),

  createFolder: (accountId: number, parent: number | null, name: string) =>
    call<number>("create_folder", { accountId, parent, name }, () => demo.createFolder(accountId, parent, name)),
  renameFolder: (mailboxId: number, name: string) => call<void>("rename_folder", { mailboxId, name }, () => demo.renameFolder(mailboxId, name)),
  deleteFolder: (mailboxId: number) => call<void>("delete_folder", { mailboxId }, () => demo.deleteFolder(mailboxId)),

  search: (query: string, accountId: number | null) => call<MessageSummary[]>("search", { query, accountId }, () => demo.search(query)),
  serverSearch: (accountId: number, query: string) =>
    call<MessageSummary[]>("server_search", { accountId, query, mailboxIds: null }, () => demo.search(query)),
  suggest: (prefix: string) => call<Contact[]>("suggest_contacts", { prefix }, () => demo.suggest(prefix)),

  send: (draft: ComposeDraft, followupSecs: number | null = null) => call<Queued>("send_message", { draft, followupSecs }, () => demo.send(draft)),
  cancelSend: (outboxId: number, messageId: string | null = null) => call<boolean>("cancel_send", { outboxId, messageId }, () => true),
  sendLater: (draft: ComposeDraft, at: number, followupSecs: number | null) => call<number>("send_later", { draft, at, followupSecs }, () => demo.sendLater(draft, at)),
  scheduled: () => call<Scheduled[]>("scheduled_messages", {}, () => demo.scheduled()),
  unschedule: (id: number) => call<ComposeDraft>("unschedule", { id }, () => demo.unschedule(id)),
  sendScheduledNow: (id: number) => call<void>("send_scheduled_now", { id }, () => void demo.unschedule(id)),
  saveDraft: (draft: ComposeDraft) => call<string>("save_draft", { draft }, () => draft.message_id ?? `demo-${Date.now()}@example.org`),
  discardDraft: (accountId: number, messageId: string) => call<void>("discard_draft", { accountId, messageId }, () => undefined),
  fileInfos: (paths: string[]) => call<FileInfo[]>("file_infos", { paths }, () => []),

  openAttachment: (id: number) => call<string>("open_attachment", { id }, () => ""),
  saveAttachment: (id: number, path: string) => call<void>("save_attachment", { id, path }, () => undefined),
  previewAttachment: (id: number) => call<{ mime_type: string; data_b64: string }>("attachment_preview", { id }, () => demo.preview()),

  inbox: (query: InboxQuery, offset: number, limit: number) => call<Page>("inbox_list", { query, offset, limit }, () => demo.inbox(query, offset, limit)),
  inboxCounts: (accountId: number | null) => call<InboxCounts>("inbox_counts", { accountId }, () => demo.inboxCounts(accountId)),
  bundle: (accountId: number | null, sender: string) => call<MessageSummary[]>("bundle_messages", { accountId, sender }, () => demo.bundle(sender)),
  setSenderCategory: (email: string, category: Category | null) =>
    call<void>("set_sender_category", { email, category }, () => demo.setSenderCategory(email, category)),
  screenerSenders: (accountId: number | null) => call<WaitingSender[]>("screener_senders", { accountId }, () => demo.screenerSenders()),
  screenerAllow: (email: string) => call<void>("screener_allow", { email }, () => void demo.screen(email, "allowed")),
  screenerBlock: (email: string) => call<Undo>("screener_block", { email }, () => demo.screen(email, "blocked")),
  screenerForget: (email: string) => call<void>("screener_forget", { email }, () => void demo.screen(email, null)),
  blockedSenders: () => call<string[]>("blocked_senders", {}, () => demo.blockedSenders()),
  subscriptions: () => call<Subscription[]>("subscriptions", {}, () => demo.subscriptions()),
  unsubscribe: (sender: string) => call<Unsubscribed>("unsubscribe", { sender }, () => demo.unsubscribe(sender)),
  unsubscribedByLink: (sender: string) => call<void>("unsubscribed_by_link", { sender }, () => undefined),

  setPinned: (ids: number[], on: boolean) => call<void>("set_pinned", { ids, on }, () => demo.mark(ids, "pinned", on)),
  setReadLater: (ids: number[], on: boolean) => call<void>("set_read_later", { ids, on }, () => demo.mark(ids, "later", on)),
  readLater: () => call<MessageSummary[]>("read_later", {}, () => demo.timeList("later")),
  snooze: (ids: number[], until: number) => call<void>("snooze", { ids, until }, () => demo.snooze(ids, until)),
  unsnooze: (ids: number[]) => call<void>("unsnooze", { ids }, () => demo.snooze(ids, null)),
  snoozed: () => call<MessageSummary[]>("snoozed", {}, () => demo.timeList("snoozed")),
  setFollowup: (ids: number[], due: number) => call<void>("set_followup", { ids, due }, () => demo.followup(ids, due)),
  dismissFollowups: (ids: number[]) => call<void>("dismiss_followups", { ids }, () => demo.followup(ids, null)),
  followups: () => call<MessageSummary[]>("followups", {}, () => demo.timeList("followups")),
  timeCounts: () => call<TimeCounts>("time_counts", {}, () => demo.timeCounts()),
  tick: () => call<void>("tick", {}, () => undefined),

  // The AI assistant (src-tauri/src/assistant.rs).
  aiRun: (id: string, request: AiRequest, offset: number) => call<AiOutcome>("ai_run", { id, request, offset }, () => demo.aiRun(request)),
  aiCancel: (id: string) => call<void>("ai_cancel", { id }, () => undefined),
  aiDestination: (accountId: number) => call<Destination>("ai_destination", { accountId }, () => demo.destination()),
  aiAskProviders: () => call<Destination[]>("ai_ask_providers", {}, () => (demo.destination().provider ? [demo.destination()] : [])),
  aiThread: (messageId: number) => call<{ summary: string | null; long: boolean }>("ai_thread", { messageId }, () => ({ summary: null, long: false })),
  aiModels: (provider: AiProvider, key: string | null) => call<string[]>("ai_models", { provider, key }, () => ["llama3.2", "qwen3:8b"]),
  aiSetKey: (providerId: string, key: string) => call<void>("ai_set_key", { providerId, key }, () => undefined),
  aiHasKey: (providerId: string) => call<boolean>("ai_has_key", { providerId }, () => false),
  aiTone: (accountId: number) => call<Tone | null>("ai_tone", { accountId }, () => null),
  aiLearnTone: (accountId: number) => call<Tone | null>("ai_learn_tone", { accountId }, () => null),
  aiForgetTone: (accountId: number) => call<void>("ai_forget_tone", { accountId }, () => undefined),
  aiForgetResults: () => call<void>("ai_forget_results", {}, () => undefined),
  setImportant: (ids: number[], important: boolean) => call<void>("set_important", { ids, important }, () => undefined),
  aiRemind: (messageId: number, due: number, note: string) => call<void>("ai_remind", { messageId, due, note }, () => undefined),
  aiCalendar: (messageId: number, item: AiItem) => call<string>("ai_calendar", { messageId, item }, () => ""),

  // Rules.
  rules: () => call<Rule[]>("rules_list", {}, () => demo.rules),
  rulesServer: (accountId: number, server: Server | null) =>
    call<RuleServer>("rules_server", { accountId, server }, () => ({ kind: "none", server: null, error: null, checked: true })),
  ruleUnsupported: (rule: Rule) => call<string | null>("rule_unsupported", { rule }, () => null),
  ruleSave: (rule: Rule) =>
    call<{ id: number; server_error: string | null }>("rule_save", { rule }, () => {
      const id = rule.id || demo.rules.length + 1;
      demo.rules = [...demo.rules.filter((r) => r.id !== id), { ...rule, id }];
      return { id, server_error: null };
    }),
  ruleDelete: (id: number) => call<string | null>("rule_delete", { id }, () => ((demo.rules = demo.rules.filter((r) => r.id !== id)), null)),
  ruleEnable: (id: number, enabled: boolean) => call<string | null>("rule_enable", { id, enabled }, () => null),
  rulesReorder: (ids: number[]) => call<string | null>("rules_reorder", { ids }, () => null),
  rulePreview: (rule: Rule) => call<{ total: number; items: MessageSummary[] }>("rule_preview", { rule }, () => ({ total: 0, items: [] })),
  ruleRun: (rule: Rule, mailboxId: number) => call<RuleReport>("rule_run", { rule, mailboxId }, () => ({ matched: 0, removed: [], accounts: [] })),

  // Templates and snippets.
  templates: () => call<Template[]>("templates_list", {}, () => demo.templates),
  templateSave: (template: Template) =>
    call<number>("template_save", { template }, () => {
      const id = template.id || demo.templates.length + 1;
      demo.templates = [...demo.templates.filter((x) => x.id !== id), { ...template, id }];
      return id;
    }),
  templateDelete: (id: number) => call<void>("template_delete", { id }, () => void (demo.templates = demo.templates.filter((x) => x.id !== id))),
  /** The texts with their variables filled ({name}, {date}, …). */
  templateRender: (texts: string[], accountId: number | null, to: string[], subject: string, date: string, time: string) =>
    call<string[]>("template_render", { texts, accountId, to, subject, date, time }, () => texts.map((x) => x.replace(/\{(datum|date)\}/gi, date).replace(/\{(uhrzeit|time)\}/gi, time))),

  // Contacts and pictures.
  contactCard: (email: string) => call<ContactCard>("contact_card", { email }, () => demo.contactCard(email)),
  addressBooks: () => call<AddressBook[]>("address_books", {}, () => []),
  carddavAdd: (name: string, url: string, username: string, password: string) => call<number>("carddav_add", { name, url, username, password }, () => 0),
  addressBookSync: (id: number) => call<void>("address_book_sync", { id }, () => undefined),
  addressBookRemove: (id: number) => call<void>("address_book_remove", { id }, () => undefined),
  microsoftContacts: (accountId: number, on: boolean) => call<void>("microsoft_contacts", { accountId, on }, () => undefined),
  picture: (email: string) => call<string | null>("picture", { email }, () => null),
  forgetPictures: (prefix: string) => call<void>("forget_pictures", { prefix }, () => undefined),

  // The attachment hub.
  hub: (filter: HubFilter, offset: number, limit: number) => call<{ total: number; offset: number; items: HubItem[] }>("hub_list", { filter, offset, limit }, () => demo.hub(filter)),
  hubSenders: (accountId: number | null) => call<HubSender[]>("hub_senders", { accountId }, () => []),

  outbox: () => call<OutboxEntry[]>("outbox_entries", {}, () => []),
  retryOutbox: (id: number) => call<void>("retry_outbox", { id }, () => undefined),
  discardOutbox: (id: number) => call<boolean>("discard_outbox", { id }, () => true),
};

/** Calls `f` for every notice of the account workers; returns the unsubscribe function. */
export function onNotice(f: (n: Notice) => void): () => void {
  if (!inApp()) return demo.onNotice(f);
  let stop: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<Notice>("mail-notice", (e) => f(e.payload)).then((un) => {
      if (cancelled) un();
      else stop = un;
    }),
  );
  return () => {
    cancelled = true;
    stop?.();
  };
}

/** Calls `f` with every piece of a streamed AI answer; returns the unsubscribe function. */
export function onAiDelta(f: (d: { id: string; text: string }) => void): () => void {
  if (!inApp()) return () => {};
  let stop: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<{ id: string; text: string }>("ai-delta", (e) => f(e.payload)).then((un) => {
      if (cancelled) un();
      else stop = un;
    }),
  );
  return () => {
    cancelled = true;
    stop?.();
  };
}

/** Calls `f` with the reminders that came due without an answer; returns the unsubscribe
 *  function. */
export function onReminders(f: (r: Reminder[]) => void): () => void {
  if (!inApp()) return () => {};
  let stop: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<Reminder[]>("reminders", (e) => f(e.payload)).then((un) => {
      if (cancelled) un();
      else stop = un;
    }),
  );
  return () => {
    cancelled = true;
    stop?.();
  };
}

/** Calls `f` with the tray menu's actions (`compose`); returns the unsubscribe function. */
export function onTrayAction(f: (action: string) => void): () => void {
  if (!inApp()) return () => {};
  let stop: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<string>("tray-action", (e) => f(e.payload)).then((un) => {
      if (cancelled) un();
      else stop = un;
    }),
  );
  return () => {
    cancelled = true;
    stop?.();
  };
}

/** Opens a link in the default browser. */
export async function openExternal(url: string): Promise<void> {
  if (inApp()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } else window.open(url, "_blank", "noopener,noreferrer");
}

/** Files to attach, picked in the system's dialog. */
export async function pickFiles(): Promise<string[]> {
  if (!inApp()) return [];
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ multiple: true, directory: false });
  if (!r) return [];
  return Array.isArray(r) ? r : [r];
}

/** Where to save a file, picked in the system's dialog (`null`: cancelled). */
export async function pickSavePath(name: string): Promise<string | null> {
  if (!inApp()) return null;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return (await save({ defaultPath: name })) ?? null;
}

/** Files dropped on the window (paths from the system, not the page): `f` gets them while
 *  `active()` says the compose sheet is open. */
export function onFileDrop(f: (paths: string[]) => void, hover: (over: boolean) => void): () => void {
  if (!inApp()) return () => {};
  let stop: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) =>
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "over" || p.type === "enter") hover(true);
        else if (p.type === "leave") hover(false);
        else if (p.type === "drop") {
          hover(false);
          if (p.paths.length) f(p.paths);
        }
      })
      .then((un) => {
        if (cancelled) un();
        else stop = un;
      }),
  );
  return () => {
    cancelled = true;
    stop?.();
  };
}
