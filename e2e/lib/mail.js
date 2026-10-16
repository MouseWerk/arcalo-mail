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
