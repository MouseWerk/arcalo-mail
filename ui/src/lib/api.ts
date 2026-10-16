// The backend: the Tauri commands (src-tauri/src/commands.rs), or the sample data when the UI
// runs in a plain browser (`npm run dev`). The UI never talks to the network itself.

import { invoke } from "@tauri-apps/api/core";
import { demo, demoAccounts, demoMailboxes } from "./demo";
import type { Account, ComposeDraft, Mailbox, MessageSummary, MessageView, Settings } from "./types";

export const inApp = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const remoteAllowed = new Set<number>();

const DEFAULT_SETTINGS: Settings = {
  version: 1,
  language: "system",
  theme: "system",
  remote_content: "ask",
  threads: true,
  mark_read_after_secs: 0,
};
let browserSettings = DEFAULT_SETTINGS;

export const api = {
  settings: (): Promise<Settings> => (inApp() ? invoke("get_settings") : Promise.resolve(browserSettings)),
  saveSettings: (settings: Settings): Promise<void> => {
    if (inApp()) return invoke("set_settings", { settings });
    browserSettings = settings;
    return Promise.resolve();
  },
  accounts: (): Promise<Account[]> => (inApp() ? invoke("list_accounts") : Promise.resolve(demoAccounts)),
  mailboxes: (accountId: number): Promise<Mailbox[]> =>
    inApp() ? invoke("list_mailboxes", { accountId }) : Promise.resolve(demoMailboxes.filter((m) => m.account_id === accountId)),
  messages: (mailboxId: number, limit = 200, offset = 0): Promise<MessageSummary[]> =>
    inApp() ? invoke("list_messages", { mailboxId, limit, offset }) : Promise.resolve(demo.messages(mailboxId)),
  message: (id: number): Promise<MessageView> =>
    inApp() ? invoke("get_message", { id }) : Promise.resolve(demo.message(id, remoteAllowed.has(id))),
  allowRemote: (id: number): Promise<void> => {
    if (inApp()) return invoke("allow_remote_content", { id });
    remoteAllowed.add(id);
    return Promise.resolve();
  },
  markSeen: (id: number, seen: boolean): Promise<void> => {
    if (inApp()) return invoke("mark_seen", { id, seen });
    demo.setSeen(id, seen);
    return Promise.resolve();
  },
  search: (query: string, accountId: number | null): Promise<MessageSummary[]> =>
    inApp() ? invoke("search", { query, accountId }) : Promise.resolve(demo.search(query)),
  send: (draft: ComposeDraft): Promise<void> => (inApp() ? invoke("send_message", { draft }) : Promise.resolve()),
};

/** Opens a link in the default browser (or mail link handling, later the compose sheet). */
export async function openExternal(url: string): Promise<void> {
  if (inApp()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } else window.open(url, "_blank", "noopener,noreferrer");
}
