// Steps in the mail window the tests and the screenshot generator share.

/** Clicks the first element of `sel` whose text is exactly `text`. */
export async function clickText(app, sel, text) {
  await app.waitUntil(
    async () => {
      for (const b of await app.$$(sel)) if ((await app.textOf(b)) === text) return true;
      return false;
    },
    10000,
    `no ${sel} "${text}"`,
  );
  for (const b of await app.$$(sel)) {
    if ((await app.textOf(b)) === text) {
      await b.click();
      return;
    }
  }
}

/** Opens a folder of an account in the sidebar (`account`: its name or address). */
export async function openFolder(app, account, name) {
  await app.waitUntil(
    async () => {
      for (const a of await app.$$(".account")) {
        if (!(await app.textOf(a)).toLowerCase().includes(account.toLowerCase())) continue;
        for (const f of await a.$$(".folder")) {
          if ((await app.textOf(f)).split("\n")[0].trim() === name) {
            await f.click();
            return true;
          }
        }
      }
      return false;
    },
    10000,
    `no folder ${name} in ${account}`,
  );
}

/** The subjects of the list's rows, top to bottom. */
export async function subjects(app) {
  const out = [];
  for (const r of await app.$$(".row .row-subject")) out.push(await app.textOf(r));
  return out;
}

/** Opens the row with this subject and waits for its message. */
export async function openMessage(app, subject, timeout = 20000) {
  await app.waitText(".row-subject", new RegExp(escape(subject)), timeout);
  for (const r of await app.$$(".row")) {
    if ((await app.textOf(r)).includes(subject)) {
      await r.click();
      await app.waitText(".message-subject", new RegExp(escape(subject)));
      await app.waitUntil(async () => (await app.$$(".message-loading")).length === 0, 15000, "message loaded");
      return;
    }
  }
  throw new Error(`no row ${subject}`);
}

/** Waits until a message with this subject is in the server's folder (or gone with `present: false`). */
export async function waitOnServer(app, server, mailbox, subject, { present = true, timeout = 20000, where = () => true } = {}) {
  await app.waitUntil(
    async () => {
      const list = (await server.state()).mailboxes[mailbox] ?? [];
      const hit = list.some((m) => m.subject === subject && !m.deleted && where(m));
      return hit === present;
    },
    timeout,
    `${subject} ${present ? "not" : "still"} in ${mailbox} on the server`,
  );
}

/** Waits until the list shows (or no longer shows) a subject. */
export async function waitRow(app, subject, present = true, timeout = 20000) {
  await app.waitUntil(async () => (await subjects(app)).includes(subject) === present, timeout, `row ${subject} ${present ? "missing" : "still there"}`);
}

const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Opens a tab of the sorted inbox (`people`, `newsletters`, `notifications`, `receipts`, `other`). */
export async function openTab(app, name) {
  await app.click(`.inbox-tab[data-tab="${name}"]`);
  await app.waitUntil(async () => {
    const open = await app.$(".inbox-tab[aria-selected='true']");
    return (await open.getAttribute("data-tab")) === name;
  }, 5000, `tab ${name}`);
}

/** The unread count on a tab of the sorted inbox, `null` when the tab is not shown. */
export async function tabCount(app, name) {
  const tabs = await app.$$(`.inbox-tab[data-tab="${name}"]`);
  if (!tabs.length) return null;
  const count = await tabs[0].$$(".inbox-tab-count");
  return count.length ? Number(await app.textOf(count[0])) : 0;
}

/** Opens an entry above the accounts („Zurückgestellt“, „Alle Posteingänge“). */
export async function openView(app, name, timeout = 15000) {
  await app.waitUntil(
    async () => {
      for (const f of await app.$$(".views .folder")) {
        if ((await app.textOf(f)).split("\n")[0].trim() === name) {
          await f.click();
          return true;
        }
      }
      return false;
    },
    timeout,
    `no view ${name}`,
  );
}

/** The count next to an entry above the accounts, `null` when it is not there. */
export async function viewCount(app, name) {
  for (const f of await app.$$(".views .folder")) {
    const [label, count] = (await app.textOf(f)).split("\n").map((s) => s.trim());
    if (label === name) return count ? Number(count) : 0;
  }
  return null;
}

/** Clicks the menu entry with this label (its time hint aside). */
export async function menuItem(app, label) {
  await clickText(app, ".menu .menu-item .menu-label", label);
}

/** A time a few seconds ahead as a `datetime-local` value (seconds included). */
export function localTime(secondsAhead) {
  const d = new Date(Date.now() + secondsAhead * 1000);
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** Chooses `value` in a `<select>` (an element or a selector) the way a person does. */
export async function choose(app, select, value) {
  const el = typeof select === "string" ? await app.waitFor(select) : select;
  await app.browser.execute(
    (e, v) => {
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(e, v);
      e.dispatchEvent(new Event("change", { bubbles: true }));
    },
    el,
    value,
  );
}

/** Opens the settings at a section of their navigation. */
export async function openSettings(app, section) {
  await app.click('.sidebar-head [aria-label="Einstellungen"]');
  await app.waitFor(".settings-nav");
  await clickText(app, ".settings-nav-item", section);
}
