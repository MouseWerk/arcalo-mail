import {
  Archive,
  Bookmark,
  CircleAlert,
  FileText,
  Flag,
  Folder,
  FolderPlus,
  Inbox,
  Mails,
  MoreHorizontal,
  Pencil,
  RefreshCw,
  Send,
  Settings as SettingsIcon,
  ShieldAlert,
  ShieldOff,
  SquarePen,
  Tag,
  Trash2,
  Upload,
  WifiOff,
} from "lucide-react";
import { useState, type ComponentType } from "react";
import { useT, type TKey } from "../lib/i18n";
import type { AccountInfo, Mailbox, Role } from "../lib/types";
import { Menu, Spinner, type MenuItem } from "./ui";

type IconType = ComponentType<{ size?: number; strokeWidth?: number; "aria-hidden"?: boolean }>;

const ROLE: Record<Role, { icon: IconType; label: TKey }> = {
  inbox: { icon: Inbox, label: "folder.inbox" },
  drafts: { icon: FileText, label: "folder.drafts" },
  sent: { icon: Send, label: "folder.sent" },
  archive: { icon: Archive, label: "folder.archive" },
  junk: { icon: ShieldAlert, label: "folder.junk" },
  trash: { icon: Trash2, label: "folder.trash" },
  all: { icon: Mails, label: "folder.all" },
  flagged: { icon: Flag, label: "folder.flagged" },
  important: { icon: Bookmark, label: "folder.important" },
};

/** Folders the app makes on the server: where snoozed messages wait (shown as „Zurückgestellt“
 *  above the accounts, not as a folder) and where the mail of turned-away senders goes. */
export const SNOOZE_FOLDER = "Snoozed";
export const SCREENED_FOLDER = "Screened Out";

/** Whether a folder is listed with the account's folders (and offered as a move target). */
export const listedFolder = (m: Mailbox) => m.selectable && !(m.role === null && m.name === SNOOZE_FOLDER);

/** The name shown for a folder: the role's name, else the last part of its path. */
export function folderLabel(m: Mailbox, t: ReturnType<typeof useT>): string {
  if (m.role) return t(ROLE[m.role].label);
  if (m.name === SCREENED_FOLDER) return t("folder.screenedOut");
  const parts = m.delimiter ? m.name.split(m.delimiter) : [m.name];
  return parts[parts.length - 1];
}

/** The name in an account's list: a folder of its own that is called like one of the account's
 *  special folders (Gmail's „[Gmail]/Entwürfe“ shows as „Entwürfe“, and a label „Entwürfe“ may
 *  exist too) is marked as a folder, so the two are told apart. */
export function listLabel(m: Mailbox, all: Mailbox[], t: ReturnType<typeof useT>): string {
  const name = folderLabel(m, t);
  if (m.role) return name;
  const key = name.trim().toLowerCase();
  const taken = all.some((o) => o.role && o.id !== m.id && listedFolder(o) && folderLabel(o, t).trim().toLowerCase() === key);
  if (!taken) return name;
  // Nested: its whole path tells it apart.
  const path = m.delimiter && m.name.includes(m.delimiter) ? m.name.split(m.delimiter).join(" / ") : name;
  return t(isGmail(all) ? "folder.sameNameLabel" : "folder.sameName", { name: path });
}

/** Gmail's folders: its own under „[Gmail]/“ (or „[Google Mail]/“ in some countries); the
 *  others are labels. */
export const isGmail = (all: Mailbox[]) => all.some((m) => /^\[(Gmail|Google Mail)\]\//.test(m.name));

export function folderIcon(m: Mailbox, gmail = false): IconType {
  if (!m.role && m.name === SCREENED_FOLDER) return ShieldOff;
  return m.role ? ROLE[m.role].icon : gmail ? Tag : Folder;
}

/** Nesting depth of a user folder („Projekte/Website“ is 1). */
const depth = (m: Mailbox) => (m.role || !m.delimiter ? 0 : m.name.split(m.delimiter).length - 1);

/** An entry above the accounts: the unified inbox, „Neue Absender“ and the time tools. */
export type ViewEntry = { id: string; label: string; icon: IconType; count: number; current: boolean; onSelect: () => void };

export type FolderAction = { kind: "create"; accountId: number; parent: Mailbox | null } | { kind: "rename"; mailbox: Mailbox } | { kind: "delete"; mailbox: Mailbox };

type Props = {
  accounts: AccountInfo[];
  mailboxes: Record<number, Mailbox[]>;
  current: number | null;
  showingOutbox: boolean;
  outboxCount: number;
  /** Something in the queue failed or was refused and waits. */
  outboxWaiting?: boolean;
  onSelect: (m: Mailbox) => void;
  onOutbox: () => void;
  onCompose: () => void;
  onSettings: () => void;
  onFolder: (a: FolderAction) => void;
  onSignIn: (a: AccountInfo) => void;
  onSync: (a: AccountInfo) => void;
  views?: ViewEntry[];
  /** With several accounts: the color (1–6) of each account's dot. */
  accountColor?: (accountId: number) => number | null;
};

function AccountStatusLine({ a, onSignIn, onSync }: { a: AccountInfo; onSignIn: () => void; onSync: () => void }) {
  const t = useT();
  const s = a.status?.status;
  if (!s || s.state === "ready" || s.state === "stopped") return null;
  if (s.state === "sign_in")
    return (
      <div className="account-status account-status-alert" role="status">
        <CircleAlert size={14} strokeWidth={1.75} aria-hidden />
        <span className="account-status-text">{t("nav.signInNeeded")}</span>
        <button type="button" className="link-btn" onClick={onSignIn}>
          {t("nav.signIn")}
        </button>
      </div>
    );
  if (s.state === "retrying" || s.state === "offline")
    return (
      <div className="account-status" role="status" title={a.status?.error ?? undefined}>
        <WifiOff size={14} strokeWidth={1.75} aria-hidden />
        <span className="account-status-text">{s.state === "retrying" ? t("nav.retrying", { n: s.in_secs }) : t("nav.offline")}</span>
        <button type="button" className="icon-btn icon-btn-xs" onClick={onSync} aria-label={t("nav.retryNow")} title={t("nav.retryNow")}>
          <RefreshCw size={12} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
    );
  return (
    <div className="account-status" role="status">
      <Spinner />
      <span className="account-status-text">{s.state === "connecting" ? t("nav.connecting") : t("nav.syncing")}</span>
    </div>
  );
}

export function Sidebar({
  accounts,
  mailboxes,
  current,
  showingOutbox,
  outboxCount,
  outboxWaiting = false,
  onSelect,
  onOutbox,
  onCompose,
  onSettings,
  onFolder,
  onSignIn,
  onSync,
  views = [],
  accountColor,
}: Props) {
  const t = useT();
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);

  const folderMenu = (m: Mailbox, at: { x: number; y: number }) => {
    const items: MenuItem[] = [{ label: t("folders.newSub"), icon: <FolderPlus size={14} aria-hidden />, onSelect: () => onFolder({ kind: "create", accountId: m.account_id, parent: m }) }];
    if (!m.role) {
      items.push({ label: t("folders.rename"), icon: <Pencil size={14} aria-hidden />, onSelect: () => onFolder({ kind: "rename", mailbox: m }) });
      items.push("separator");
      items.push({ label: t("folders.delete"), icon: <Trash2 size={14} aria-hidden />, danger: true, onSelect: () => onFolder({ kind: "delete", mailbox: m }) });
    }
    setMenu({ items, at });
  };

  return (
    <nav className="sidebar" aria-label={t("nav.label")}>
      <div className="sidebar-head">
        <span className="brand">{t("app.name")}</span>
        <button type="button" className="icon-btn" onClick={onSettings} aria-label={t("settings.title")} title={t("settings.titleWithKey")}>
          <SettingsIcon size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      <div className="sidebar-compose">
        <button type="button" className="btn btn-primary compose-btn" onClick={onCompose} title={t("nav.composeKey")}>
          <SquarePen size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("nav.compose")}</span>
        </button>
      </div>
      <div className="sidebar-scroll">
        {accounts.length === 0 && <p className="sidebar-empty">{t("nav.noAccount")}</p>}
        {views.length > 0 && (
          <section className="account views" aria-label={t("nav.views")}>
            <ul className="folders">
              {views.map((v) => {
                const Icon = v.icon;
                return (
                  <li key={v.id} className="folder-item">
                    <button type="button" className="folder" data-view={v.id} aria-current={v.current ? "page" : undefined} onClick={v.onSelect}>
                      <Icon size={16} strokeWidth={1.75} aria-hidden />
                      <span className="folder-name">{v.label}</span>
                      {v.count > 0 && <span className="count">{v.count}</span>}
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        )}
        {accounts.map((a) => (
          <section key={a.id} className="account" aria-label={a.name || a.email}>
            <div className="account-head">
              {accountColor?.(a.id) != null && <span className="account-dot" data-color={accountColor(a.id) ?? undefined} aria-hidden />}
              <h2 className="eyebrow" title={a.email}>
                {a.name || a.email}
              </h2>
              <button
                type="button"
                className="icon-btn icon-btn-xs account-more"
                aria-label={t("folders.new")}
                title={t("folders.new")}
                onClick={() => onFolder({ kind: "create", accountId: a.id, parent: null })}
              >
                <FolderPlus size={14} strokeWidth={1.75} aria-hidden />
              </button>
            </div>
            <AccountStatusLine a={a} onSignIn={() => onSignIn(a)} onSync={() => onSync(a)} />
            <ul className="folders">
              {(mailboxes[a.id] ?? [])
                .filter(listedFolder)
                .map((m, i, shown) => {
                  const gmail = isGmail(shown);
                  const Icon = folderIcon(m, gmail);
                  const isCurrent = m.id === current && !showingOutbox;
                  const count = m.role === "drafts" || m.role === "sent" || m.role === "trash" ? 0 : m.unread;
                  // The account's own folders (or Gmail's labels) below its special ones.
                  const own = !m.role && i > 0 && !!shown[i - 1].role;
                  return [
                    own && (
                      <li key={`own-${m.id}`} className="folder-group" role="presentation">
                        {t(gmail ? "folder.labels" : "folder.own")}
                      </li>
                    ),
                    <li key={m.id} className="folder-item">
                      <button
                        type="button"
                        className="folder"
                        data-role={m.role ?? undefined}
                        aria-current={isCurrent ? "page" : undefined}
                        style={{ paddingInlineStart: `calc(var(--sp-2) + ${depth(m)} * var(--sp-4))` }}
                        onClick={() => onSelect(m)}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          folderMenu(m, { x: e.clientX, y: e.clientY });
                        }}
                      >
                        <Icon size={16} strokeWidth={1.75} aria-hidden />
                        <span className="folder-name">{listLabel(m, mailboxes[a.id] ?? [], t)}</span>
                        {m.sync_error && (
                          <span className="folder-alert" title={t("folder.syncError", { error: m.sync_error })} aria-label={t("folder.syncError", { error: m.sync_error })}>
                            <CircleAlert size={14} strokeWidth={1.75} aria-hidden />
                          </span>
                        )}
                        {count > 0 && (
                          <span className="count" aria-label={t("nav.unread", { n: count })}>
                            {count}
                          </span>
                        )}
                      </button>
                      <button
                        type="button"
                        className="icon-btn icon-btn-xs folder-more"
                        aria-label={t("folders.actions", { name: folderLabel(m, t) })}
                        onClick={(e) => {
                          const r = e.currentTarget.getBoundingClientRect();
                          folderMenu(m, { x: r.left, y: r.bottom + 4 });
                        }}
                      >
                        <MoreHorizontal size={14} strokeWidth={1.75} aria-hidden />
                      </button>
                    </li>,
                  ];
                })}
            </ul>
          </section>
        ))}
        {outboxCount > 0 && (
          <section className="account">
            <ul className="folders">
              <li className="folder-item">
                <button type="button" className="folder" aria-current={showingOutbox ? "page" : undefined} onClick={onOutbox} title={outboxWaiting ? t("outbox.someWait") : undefined}>
                  {outboxWaiting ? <CircleAlert size={16} strokeWidth={1.75} aria-hidden className="folder-alert-icon" /> : <Upload size={16} strokeWidth={1.75} aria-hidden />}
                  <span className="folder-name">{t("folder.outbox")}</span>
                  <span className="count">{outboxCount}</span>
                </button>
              </li>
            </ul>
          </section>
        )}
      </div>
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("folders.menu")} />}
    </nav>
  );
}
