import { Archive, FileText, Flag, Folder, Inbox, Mails, Send, ShieldAlert, SquarePen, Trash2 } from "lucide-react";
import type { ComponentType } from "react";
import { useT, type TKey } from "../lib/i18n";
import type { Account, Mailbox, Role } from "../lib/types";
import { Preferences } from "./Preferences";
import type { LanguageChoice, ThemeChoice } from "../lib/types";

const ROLE: Record<Role, { icon: ComponentType<{ size?: number; strokeWidth?: number }>; label: TKey }> = {
  inbox: { icon: Inbox, label: "folder.inbox" },
  drafts: { icon: FileText, label: "folder.drafts" },
  sent: { icon: Send, label: "folder.sent" },
  archive: { icon: Archive, label: "folder.archive" },
  junk: { icon: ShieldAlert, label: "folder.junk" },
  trash: { icon: Trash2, label: "folder.trash" },
  all: { icon: Mails, label: "folder.all" },
  flagged: { icon: Flag, label: "folder.flagged" },
};

/** The name shown for a folder: the role's name, else the last part of its path. */
export function folderLabel(m: Mailbox, t: ReturnType<typeof useT>): string {
  if (m.role) return t(ROLE[m.role].label);
  const parts = m.delimiter ? m.name.split(m.delimiter) : [m.name];
  return parts[parts.length - 1];
}

/** Nesting depth of a user folder („Projekte/Website“ is 1). */
const depth = (m: Mailbox) => (m.role || !m.delimiter ? 0 : m.name.split(m.delimiter).length - 1);

type Props = {
  accounts: Account[];
  mailboxes: Record<number, Mailbox[]>;
  current: number | null;
  onSelect: (m: Mailbox) => void;
  onCompose: () => void;
  theme: ThemeChoice;
  language: LanguageChoice;
  onTheme: (t: ThemeChoice) => void;
  onLanguage: (l: LanguageChoice) => void;
};

export function Sidebar({ accounts, mailboxes, current, onSelect, onCompose, theme, language, onTheme, onLanguage }: Props) {
  const t = useT();
  return (
    <nav className="sidebar" aria-label={t("nav.label")}>
      <div className="sidebar-head">
        <span className="brand">{t("app.name")}</span>
        <button type="button" className="btn btn-primary compose-btn" onClick={onCompose} title={t("nav.compose")}>
          <SquarePen size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("nav.compose")}</span>
        </button>
      </div>
      <div className="sidebar-scroll">
        {accounts.length === 0 && <p className="sidebar-empty">{t("nav.noAccount")}</p>}
        {accounts.map((a) => (
          <section key={a.id} className="account">
            <h2 className="eyebrow" title={a.email}>
              {a.name || a.email}
            </h2>
            <ul className="folders">
              {(mailboxes[a.id] ?? [])
                .filter((m) => m.selectable)
                .map((m) => {
                  const Icon = m.role ? ROLE[m.role].icon : Folder;
                  const isCurrent = m.id === current;
                  return (
                    <li key={m.id}>
                      <button
                        type="button"
                        className="folder"
                        aria-current={isCurrent ? "true" : undefined}
                        style={{ paddingInlineStart: `calc(var(--sp-2) + ${depth(m)} * var(--sp-4))` }}
                        onClick={() => onSelect(m)}
                      >
                        <Icon size={16} strokeWidth={1.75} aria-hidden />
                        <span className="folder-name">{folderLabel(m, t)}</span>
                        {m.unread > 0 && (
                          <span className="count" aria-label={t("nav.unread", { n: m.unread })}>
                            {m.unread}
                          </span>
                        )}
                      </button>
                    </li>
                  );
                })}
            </ul>
          </section>
        ))}
      </div>
      <Preferences theme={theme} language={language} onTheme={onTheme} onLanguage={onLanguage} />
    </nav>
  );
}
