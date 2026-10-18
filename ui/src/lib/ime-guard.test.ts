// Fails on a key handler that acts on Enter or Escape without asking lib/ime.ts first: during an
// input-method composition (Japanese, Chinese, Korean, dead keys on a Mac) those keys pick the
// candidate or cancel the composition and must not submit a field, rename a page or close a
// dialog. WebKit on macOS sends that Enter with `isComposing` false and keyCode 229, so a check of
// `e.nativeEvent.isComposing` alone is not enough: `isComposing(e)` or `isKey(e, "Enter")` is.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

const SRC = path.resolve(__dirname, "..");
/** Not handlers: they parse key events for the shortcut settings. */
const SKIP = new Set(["lib/shortcuts.ts", "lib/ime.ts"]);

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "locales" ? [] : sources(p);
    return /\.(tsx|ts)$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });
}

const KEYS = new Set(["Enter", "Escape"]);
const isFn = (n: ts.Node): n is ts.FunctionLikeDeclaration =>
  ts.isArrowFunction(n) || ts.isFunctionExpression(n) || ts.isFunctionDeclaration(n) || ts.isMethodDeclaration(n);

/** `x.key === "Enter"`, `switch (x.key) { case "Escape": … }` or `["Enter", …].includes(x.key)`. */
function comparesKey(n: ts.Node): boolean {
  if (ts.isBinaryExpression(n) && [ts.SyntaxKind.EqualsEqualsEqualsToken, ts.SyntaxKind.ExclamationEqualsEqualsToken, ts.SyntaxKind.EqualsEqualsToken].includes(n.operatorToken.kind))
    return [[n.left, n.right], [n.right, n.left]].some(([x, y]) => ts.isPropertyAccessExpression(x) && x.name.text === "key" && ts.isStringLiteral(y) && KEYS.has(y.text));
  if (ts.isSwitchStatement(n) && ts.isPropertyAccessExpression(n.expression) && n.expression.name.text === "key")
    return n.caseBlock.clauses.some((c) => ts.isCaseClause(c) && ts.isStringLiteral(c.expression) && KEYS.has(c.expression.text));
  if (ts.isCallExpression(n) && ts.isPropertyAccessExpression(n.expression) && n.expression.name.text === "includes" && ts.isArrayLiteralExpression(n.expression.expression)) {
    const arg = n.arguments[0];
    return !!arg && ts.isPropertyAccessExpression(arg) && arg.name.text === "key" && n.expression.expression.elements.some((el) => ts.isStringLiteral(el) && KEYS.has(el.text));
  }
  return false;
}

function unguarded(file: string): string[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  if (SKIP.has(rel)) return [];
  const sf = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const flagged = new Set<ts.Node>();
  const visit = (n: ts.Node, fn: ts.FunctionLikeDeclaration | null) => {
    if (fn && comparesKey(n)) flagged.add(fn);
    ts.forEachChild(n, (c) => visit(c, isFn(n) ? n : fn));
  };
  visit(sf, null);
  return [...flagged]
    .filter((fn) => !/\b(isComposing|isKey)\(/.test((fn as ts.FunctionLikeDeclaration).body?.getText(sf) ?? ""))
    .map((fn) => `${rel}:${sf.getLineAndCharacterOfPosition(fn.getStart(sf)).line + 1}`);
}

describe("IME-safe Enter and Escape", () => {
  it("every handler of Enter or Escape asks isComposing (or uses isKey)", () => {
    expect(sources(SRC).flatMap(unguarded)).toEqual([]);
  }, 60_000);
});
