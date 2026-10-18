// Fails on icon-only buttons without an accessible name: a <button> (or an element with
// role="button") whose content is only icons must carry aria-label or aria-labelledby. Text in
// the button, a text expression (`{t("…")}`, `{name}`) or a spread of props (the caller passes
// the label) count as a name. ALLOWED lists the few buttons whose name comes from elsewhere.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

const SRC = path.resolve(__dirname, "..");

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "locales" ? [] : sources(p);
    return e.name.endsWith(".tsx") && !/\.test\.tsx$/.test(e.name) ? [p] : [];
  });
}

/** `file:line` of buttons that are named another way (none yet). */
const ALLOWED = new Set<string>([]);

const NAME_ATTRS = new Set(["aria-label", "aria-labelledby"]);

/** Icon components of the file being scanned: lucide imports, PageIcon, `Icon` props. */
let icons = new Set<string>();
const isIcon = (tag: string) => icons.has(tag) || /^(Icon|PageIcon|[A-Z]\w*Icon)$/.test(tag);

/** Whether JSX children render text (anything but icon components and empty markup). */
function hasText(nodes: readonly ts.Node[]): boolean {
  return nodes.some((n) => {
    if (ts.isJsxText(n)) return n.text.trim() !== "";
    if (ts.isJsxExpression(n)) return n.expression ? exprHasText(n.expression) : false;
    if (ts.isJsxSelfClosingElement(n)) return selfClosingHasText(n);
    if (ts.isJsxElement(n)) return hasText(n.children);
    if (ts.isJsxFragment(n)) return hasText(n.children);
    return false;
  });
}

function selfClosingHasText(n: ts.JsxSelfClosingElement): boolean {
  const tag = n.tagName.getText();
  // An image with alt text names the button; a component in lowercase is markup.
  if (tag === "img") return n.attributes.properties.some((p) => ts.isJsxAttribute(p) && p.name.getText() === "alt");
  // Icons and plain markup are text-free; other components (chips, labels) render text.
  return /^[A-Z]/.test(tag) && !isIcon(tag);
}

function exprHasText(e: ts.Expression): boolean {
  if (ts.isParenthesizedExpression(e)) return exprHasText(e.expression);
  if (ts.isJsxSelfClosingElement(e)) return selfClosingHasText(e);
  if (ts.isJsxElement(e) || ts.isJsxFragment(e)) return hasText(e.children);
  if (ts.isConditionalExpression(e)) return exprHasText(e.whenTrue) || exprHasText(e.whenFalse);
  if (ts.isBinaryExpression(e) && [ts.SyntaxKind.AmpersandAmpersandToken, ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken].includes(e.operatorToken.kind))
    return e.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken ? exprHasText(e.right) : exprHasText(e.left) || exprHasText(e.right);
  if (e.kind === ts.SyntaxKind.NullKeyword || e.kind === ts.SyntaxKind.FalseKeyword || e.kind === ts.SyntaxKind.UndefinedKeyword) return false;
  if (ts.isIdentifier(e) && e.text === "undefined") return false;
  // `<Icon size={14} />` stored in a capitalized variable and rendered as {icon}: still text-free
  // only if it is clearly an element; anything else (strings, calls, names) may be text.
  return true;
}

function scan(file: string): string[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  const sf = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  icons = new Set(
    sf.statements.flatMap((st) =>
      ts.isImportDeclaration(st) && st.moduleSpecifier.getText(sf).includes("lucide-react") && st.importClause?.namedBindings && ts.isNamedImports(st.importClause.namedBindings)
        ? st.importClause.namedBindings.elements.map((e) => e.name.text)
        : [],
    ),
  );
  const check = (opening: ts.JsxOpeningLikeElement, children: readonly ts.Node[]) => {
    const tag = opening.tagName.getText(sf);
    const attrs = opening.attributes.properties;
    const role = attrs.find((p) => ts.isJsxAttribute(p) && p.name.getText(sf) === "role");
    const isButton = tag === "button" || tag === "Button" || (/^[a-z]/.test(tag) && role && ts.isJsxAttribute(role) && role.initializer?.getText(sf).replace(/["'{}]/g, "") === "button");
    if (!isButton) return;
    if (attrs.some((p) => ts.isJsxSpreadAttribute(p) || NAME_ATTRS.has(p.name.getText(sf)) || p.name.getText(sf) === "hidden")) return;
    if (hasText(children)) return;
    const where = `${rel}:${sf.getLineAndCharacterOfPosition(opening.getStart()).line + 1}`;
    if (!ALLOWED.has(where)) out.push(where);
  };
  const visit = (n: ts.Node): void => {
    if (ts.isJsxElement(n)) check(n.openingElement, n.children);
    else if (ts.isJsxSelfClosingElement(n)) check(n, []);
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

describe("accessible names", () => {
  it("icon-only buttons have an aria-label", () => {
    expect(sources(SRC).flatMap(scan)).toEqual([]);
  }, 30_000);
});
