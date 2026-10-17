// Fails on a button that opens a menu without saying so: screen readers announce „Menü“ (and,
// while it is open, „erweitert“) only with aria-haspopup on the trigger. useMenu's `openAt` sets
// aria-expanded while the menu is open; aria-haspopup is written on the element.

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

/** An onClick that opens a menu at the element (not a context menu, not a submenu item). */
const OPENS_MENU = /\b(open\w*Menu\w*|set\w*Menu|(?!close|hide)\w+Menu)\(\s*(?!null|false|undefined)/;

function offenders(file: string): string[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  const sf = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  const visit = (n: ts.Node) => {
    if (ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n)) {
      const props = n.attributes.properties;
      const click = props.find((p) => ts.isJsxAttribute(p) && p.name.getText(sf) === "onClick");
      if (click && ts.isJsxAttribute(click) && OPENS_MENU.test(click.initializer?.getText(sf) ?? "")) {
        const named = props.some((p) => ts.isJsxSpreadAttribute(p) || (ts.isJsxAttribute(p) && p.name.getText(sf) === "aria-haspopup"));
        if (!named) out.push(`${rel}:${sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1}`);
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

/** Roles a <button> may take without hiding that it is activatable. */
const BUTTON_ROLES = new Set(["button", "combobox", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "switch", "radio", "checkbox", "link", "gridcell"]);

function wrongRoles(file: string): string[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  const sf = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  const visit = (n: ts.Node) => {
    if ((ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n)) && n.tagName.getText(sf) === "button") {
      const role = n.attributes.properties.find((p) => ts.isJsxAttribute(p) && p.name.getText(sf) === "role");
      const value = role && ts.isJsxAttribute(role) && role.initializer && ts.isStringLiteral(role.initializer) ? role.initializer.text : null;
      if (value && !BUTTON_ROLES.has(value)) out.push(`${rel}:${sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1} role=${value}`);
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

describe("menu buttons", () => {
  it("every button that opens a menu has aria-haspopup", () => {
    expect(sources(SRC).flatMap(offenders)).toEqual([]);
  }, 60_000);

  it("no button takes a role that hides that it is one (a list item wraps it instead)", () => {
    expect(sources(SRC).flatMap(wrongRoles)).toEqual([]);
  }, 60_000);
});
