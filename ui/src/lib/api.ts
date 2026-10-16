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

  send: (draft: ComposeDraft) => call<Queued>("send_message", { draft }, () => demo.send(draft)),
  cancelSend: (outboxId: number) => call<boolean>("cancel_send", { outboxId }, () => true),
  saveDraft: (draft: ComposeDraft) => call<string>("save_draft", { draft }, () => draft.message_id ?? `demo-${Date.now()}@example.org`),
  discardDraft: (accountId: number, messageId: string) => call<void>("discard_draft", { accountId, messageId }, () => undefined),
  fileInfos: (paths: string[]) => call<FileInfo[]>("file_infos", { paths }, () => []),

  openAttachment: (id: number) => call<string>("open_attachment", { id }, () => ""),
  saveAttachment: (id: number, path: string) => call<void>("save_attachment", { id, path }, () => undefined),
  previewAttachment: (id: number) => call<{ mime_type: string; data_b64: string }>("attachment_preview", { id }, () => demo.preview()),

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
