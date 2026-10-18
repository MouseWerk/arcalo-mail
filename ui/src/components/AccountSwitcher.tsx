// Switching between accounts, as Gmail and Spark do: „Alle“ and each account as a chip with its
// color and unread count, in the list's header (and at the top of the folders on a phone). A
// tap shows that account's folder, „Alle“ every account's together; on a phone a swipe over the
// list's header goes to the next or the previous one.

import { Layers } from "lucide-react";
import type { ReactNode } from "react";
import { useT } from "../lib/i18n";
import type { AccountInfo } from "../lib/types";

export type SwitcherAccount = { id: number; name: string; color: number | null; unread: number };

/** The account after or before `current` (`null`: all), round the list. */
export function stepAccount(ids: number[], current: number | null, dir: 1 | -1): number | null {
  const order: (number | null)[] = [null, ...ids];
  const i = Math.max(0, order.indexOf(current));
  return order[(i + dir + order.length) % order.length];
}

/** The switcher's accounts: their names, colors and unread mail in the inbox. */
export function switcherAccounts(accounts: AccountInfo[], unreadOf: (id: number) => number, colorOf: (id: number) => number | null): SwitcherAccount[] {
  return accounts.map((a) => ({ id: a.id, name: a.name || a.email, color: colorOf(a.id), unread: unreadOf(a.id) }));
}

export function AccountSwitcher({ accounts, current, total, onPick, label }: { accounts: SwitcherAccount[]; current: number | null; total: number; onPick: (id: number | null) => void; label?: string }) {
  const t = useT();
  const chip = (id: number | null, name: string, mark: ReactNode, unread: number) => (
    <button
      key={id ?? "all"}
      type="button"
      role="tab"
      className="acct-chip"
      data-account={id ?? "all"}
      aria-selected={current === id}
      aria-label={unread > 0 ? `${name}, ${t("nav.unread", { n: unread })}` : name}
      onClick={() => onPick(id)}
    >
      {mark}
      <span className="acct-chip-name">{name}</span>
      {unread > 0 && (
        <span className="acct-chip-count" aria-hidden>
          {unread}
        </span>
      )}
    </button>
  );
  return (
    <div className="acct-switch" role="tablist" aria-label={label ?? t("accounts.switch")}>
      {chip(null, t("accounts.all"), <Layers size={14} strokeWidth={1.75} aria-hidden />, total)}
      {accounts.map((a) =>
        chip(
          a.id,
          a.name,
          <span className="acct-chip-mark" data-color={a.color ?? undefined} aria-hidden>
            {(a.name.trim()[0] ?? "?").toUpperCase()}
          </span>,
          a.unread,
        ),
      )}
    </div>
  );
}
