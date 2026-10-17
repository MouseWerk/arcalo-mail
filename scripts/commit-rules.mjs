// Commit rules (CLAUDE.md, owner decisions): commits are authored by the owner, with no co-author
// or session trailers and no "Generated with" line. CI runs it on the commits of each push
// (`github.event.commits`, handed over as JSON in COMMITS); Arcalo keeps these rules in text only.
//
//   COMMITS='[{"id":"…","message":"…","author":{"name":"…","email":"…"}}]' node scripts/commit-rules.mjs

const OWNER = { name: "Maurice Kleindienst", email: "kleindiema@gmail.com" };
/** Accounts that may commit besides the owner (none so far). */
const BOTS = new Set([]);
const TRAILER = /^(co-authored-by|[\w-]+-session|generated-by|assisted-by)\s*:/im;
const GENERATED = /^.*\bgenerated (with|by) \[/im;

/** What is wrong with the commits (empty: nothing). */
export function problems(commits) {
  const out = [];
  for (const c of commits ?? []) {
    const id = String(c.id ?? "").slice(0, 7);
    const email = c.author?.email ?? "";
    if (!BOTS.has(email) && (email !== OWNER.email || c.author?.name !== OWNER.name)) out.push(`${id}: author ${c.author?.name} <${email}>, expected ${OWNER.name} <${OWNER.email}>`);
    if (TRAILER.test(c.message ?? "")) out.push(`${id}: no co-author or session trailers`);
    if (GENERATED.test(c.message ?? "")) out.push(`${id}: no "Generated with" line`);
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const found = problems(JSON.parse(process.env.COMMITS || "[]"));
  for (const p of found) console.error(`::error::${p}`);
  process.exit(found.length ? 1 : 0);
}
