// Conventions of the e2e suite (CLAUDE.md), checked in the test files themselves: every test
// leaves a screenshot when it fails (`guarded`), no test is skipped or narrowed with `.only`, the
// app comes only from ARCALO_MAIL_APP (the harness), processes are never killed by name or
// pattern, a reload goes through `app.reload()`, and nothing talks to a real mail server.
//
//   node --test scripts/e2e-conventions.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const E2E = path.join(ROOT, "e2e");

function files(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "node_modules" || e.name === "screenshots" ? [] : files(p);
    return /\.(m?js|sh)$/.test(e.name) ? [p] : [];
  });
}

const all = files(E2E);
const tests = all.filter((f) => /\.test\.m?js$/.test(f));
const rel = (f) => path.relative(ROOT, f);
const lines = (f) => fs.readFileSync(f, "utf8").split("\n");
const code = (l) => !/^\s*(\/\/|\*|\/\*)/.test(l);

/** Lines of `list` matching `re` (comments left out), as `file:line: text`. */
function hits(list, re) {
  return list.flatMap((f) => lines(f).flatMap((l, i) => (code(l) && re.test(l) ? [`${rel(f)}:${i + 1}: ${l.trim()}`] : [])));
}

test("the suite is found", () => {
  assert.ok(tests.length > 10, `only ${tests.length} test files`);
});

test("every test file wraps its tests in guarded() (a screenshot on failure)", () => {
  const bare = tests.filter((f) => /^\s*test\(/m.test(fs.readFileSync(f, "utf8")) && !/guarded\(/.test(fs.readFileSync(f, "utf8")));
  assert.deepEqual(bare.map(rel), []);
});

test("no test is skipped, narrowed or left as a todo", () => {
  assert.deepEqual(hits(tests, /\b(test|it|describe|suite)\.(skip|only|todo)\(|\{\s*(skip|only|todo)\s*:\s*(true|["'`])/), []);
});

test("the app comes only from ARCALO_MAIL_APP (through the harness)", () => {
  const outside = all.filter((f) => !/e2e\/lib\/harness\.js$/.test(f) && !/\.sh$/.test(f));
  assert.deepEqual(hits(outside, /target\/(debug|release)\/arcalo-mail(?!-testserver)\b/), []);
});

test("processes are killed only by the PIDs the run started", () => {
  assert.deepEqual(hits(all, /\b(pkill|killall|taskkill\s+\/IM)\b|kill\s+\$\(pgrep|xargs\s+kill/), []);
});

test("a reload goes through app.reload()", () => {
  const outside = all.filter((f) => !/e2e\/lib\/harness\.js$/.test(f));
  assert.deepEqual(hits(outside, /location\.reload\(|browser\.refresh\(/), []);
});

test("accounts talk to the local test server only", () => {
  // Real providers' hosts in an account set-up would send a test's password over the internet.
  assert.deepEqual(hits(tests, /\b(imap|smtp|pop)\.(gmail|googlemail|outlook|office365|web|gmx|t-online|mail\.yahoo|aol|mail\.me)\.(com|de|net)\b/), []);
});
