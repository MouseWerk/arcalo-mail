// Fails on user-facing text written into the code instead of the catalogs (`ui/src/locales`), as in Arcalo:
// JSX text, text props (title, aria-label, placeholder, label, …), text fields of objects
// (label, title, message, …), texts passed to toasts, and any string with German letters.
// Technical strings (names, formats, syntax the user types) are listed in ALLOWED.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

const SRC = path.resolve(__dirname, "..");

function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "locales" ? [] : sources(p);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) && !e.name.endsWith(".d.ts") ? [p] : [];
  });
}

/** JSX props that show text. */
const TEXT_PROPS = new Set([
  "title",
  "aria-label",
  "aria-description",
  "aria-roledescription",
  "aria-valuetext",
  "placeholder",
  "label",
  "description",
  "alt",
  "data-tooltip",
  "confirmLabel",
  "cancelLabel",
  "message",
  "intro",
  "hint",
  "heading",
  "subtitle",
  "emptyText",
  "empty",
  "tooltip",
  "detail",
]);
/** Object fields that hold text shown to the user. */
const TEXT_FIELDS = new Set(["label", "title", "description", "message", "placeholder", "hint", "confirmLabel", "cancelLabel", "altLabel", "detail", "tooltip", "heading", "intro", "subtitle", "emptyText", "help", "sub", "desc", "reason", "aria-label"]);
/** DOM properties that show text when assigned. */
const DOM_TEXT = new Set(["textContent", "innerText", "title", "alt", "placeholder", "ariaLabel", "error"]);
/** Functions whose first argument is shown as a message. */
const MESSAGE_CALLS = new Set(["toast", "success", "info", "warning", "warn", "notify", "alert", "setError", "setStatus", "setMessage", "setHint", "setNotice", "error", "withHint"]);

const GERMAN = /[äöüÄÖÜß„“‚‘]/;
/** German words without umlauts that only a German UI text has („3 Seiten“, „gebucht“). */
const GERMAN_WORDS = /(?<![\p{L}\p{N}_./-])(Seite|Seiten|Aufgabe|Aufgaben|Notiz|Notizen|Datei|Dateien|Termin|Termine|Eintrag|Stunde|Stunden|Minute|Minuten|Woche|Wochen|Gebucht|gebucht|Nachricht|Nachrichten|Ordner|Konto|Konten|Postfach|Antwort|Absender|Betreff|Anhang|Entwurf|Entwürfe|nicht|keine|Keine|wird|werden)(?![\p{L}\p{N}_])/u;
const LETTERS = /[A-Za-zÄÖÜäöüß]{2,}/;
/** i18n keys, CSS classes, identifiers, paths and other technical strings. */
const TECHNICAL = [
  /^[a-z][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+$/, // i18n key
  /^[a-z0-9_-]+$/, // id, class name, token
  /^[\w.-]+\/[\w./-]*$/, // path or mime type
  /^(https?|webcal):\/\//, // URLs and URL placeholders: webcal://…
  /^-----BEGIN [A-Z ]+-----$/, // the armor line of a key, as a placeholder
  /^&[a-z]+;$/, // HTML entities
  /^(Ctrl|Alt|Shift|Mod|Cmd|Meta)\b[+\w ]*$/, // shortcuts
  /^\{\}\+(Enter|Tab|[A-Z]|⇧\+[A-Z])$/, // a modifier (filled in) and a key: {}+Enter
  /^[A-Z][A-Z0-9_-]*$/, // codes and placeholders: CODE, AET-12, NP-8801
  /^\{\}\/[\w./-]+$/, // URL paths after a value: {}/v1/models
  /^#[\w-]+$/, // tags: #privat
  /^[a-z0-9]+[-_]…$/, // token prefixes: sk-…, ghp_…
  /^[a-z][\w.-]*\*$/, // model name patterns: gpt-4o*
  /^[\w.:/-]+(, [\w.:/-]+)+$/, // lists of model names: gpt-4o, text-embedding-3-small
  /^[\w.+-]+@[\w.-]+$/, // addresses: name@example.org
  /^[\w.-]+\.[a-z]{2,}(:\d+)?$/, // host names: imap.example.org:993
];

/** Proper names and technical words that read the same in both languages. */
const NAMES = new Set([
  "Arcalo",
  "Arcalo Mail",
  // Key caps, and the language names, which are written in their own language.
  "Enter",
  "Esc",
  "Tab",
  "Deutsch",
  "English",
  // Protocols, providers and formats a mail app names as they are.
  "IMAP",
  "SMTP",
  "POP3",
  "JMAP",
  "CalDAV",
  "CardDAV",
  "ManageSieve",
  "Sieve",
  "OAuth",
  "TLS",
  "STARTTLS",
  "SSL/TLS",
  "Gmail",
  "Google",
  "Google Workspace",
  "Microsoft",
  "Microsoft 365",
  "Outlook",
  "Outlook.com",
  "Exchange",
  "iCloud",
  "Yahoo",
  "AOL",
  "GMX",
  "WEB.DE",
  "T-Online",
  "Fastmail",
  "Posteo",
  "mailbox.org",
  "Nextcloud",
  "Teams",
  "Zoom",
  "Webex",
  "Google Meet",
  "Jitsi",
  "BIMI",
  "Gravatar",
  "PGP",
  "OpenPGP",
  "S/MIME",
  "Autocrypt",
  "Ollama",
  "LM Studio",
  "OpenAI",
  "Anthropic",
  "Mistral",
  "Groq",
  "OpenRouter",
  "PDF",
  "HTML",
  "CSV",
  "JSON",
  "ICS",
  "URL",
  "OK",
  "ID",
  "API",
  "HTTP",
  "HTTPS",
  "Cc",
  "Bcc",
  "Windows",
  "macOS",
  "Linux",
  "Android",
  "iOS",
  // Font names and format patterns.
  "Inter",
  "JetBrains Mono",
  "HH:MM",
]);

/**
 * Allowed per file: `file` is relative to `ui/src`, `text` matches the whole string. Each entry
 * says why (typed syntax, file formats, sample data, logs).
 */
const ALLOWED: { file: string | RegExp; text: RegExp; why: string }[] = [
  { file: "lib/demo.ts", text: /./, why: "sample mailbox of the plain-browser preview (npm run dev), never part of the app" },
];

interface Hit {
  file: string;
  line: number;
  text: string;
}

function technical(s: string) {
  const v = s.trim();
  if (!LETTERS.test(v)) return true;
  if (NAMES.has(v)) return true;
  return TECHNICAL.some((r) => r.test(v));
}

function allowed(file: string, text: string) {
  return ALLOWED.some((a) => (typeof a.file === "string" ? a.file === file : a.file.test(file)) && a.text.test(text));
}

function literalText(n: ts.Node): string | null {
  if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) return n.text;
  if (ts.isTemplateExpression(n)) return [n.head.text, ...n.templateSpans.map((s) => s.literal.text)].join("{}");
  return null;
}

/** String literals directly in an expression (through ?:, ||, ??, parentheses). */
function literalsIn(n: ts.Node | undefined): ts.Node[] {
  if (!n) return [];
  if (ts.isParenthesizedExpression(n)) return literalsIn(n.expression);
  if (ts.isConditionalExpression(n)) return [...literalsIn(n.whenTrue), ...literalsIn(n.whenFalse)];
  if (ts.isBinaryExpression(n) && [ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken, ts.SyntaxKind.PlusToken].includes(n.operatorToken.kind))
    return [...literalsIn(n.left), ...literalsIn(n.right)];
  if (ts.isJsxExpression(n)) return literalsIn(n.expression);
  // `${n} ${n === 1 ? "Seite" : "Seiten"}`: the template itself and the texts in its parts.
  if (ts.isTemplateExpression(n)) return [n, ...n.templateSpans.flatMap((s) => literalsIn(s.expression))];
  if (ts.isArrayLiteralExpression(n)) return n.elements.flatMap((e) => literalsIn(e));
  return literalText(n) !== null ? [n] : [];
}

function scan(file: string): Hit[] {
  const rel = path.relative(SRC, file).split(path.sep).join("/");
  const text = fs.readFileSync(file, "utf8");
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const hits: Hit[] = [];
  const seen = new Set<number>();
  const flag = (n: ts.Node, s: string) => {
    if (seen.has(n.getStart()) || technical(s) || allowed(rel, s.trim())) return;
    seen.add(n.getStart());
    hits.push({ file: rel, line: sf.getLineAndCharacterOfPosition(n.getStart()).line + 1, text: s.trim().slice(0, 90) });
  };
  const visit = (n: ts.Node): void => {
    if (ts.isJsxText(n)) {
      const s = n.text.replace(/\s+/g, " ");
      if (LETTERS.test(s)) flag(n, s);
    } else if (ts.isJsxExpression(n) && n.parent && (ts.isJsxElement(n.parent) || ts.isJsxFragment(n.parent))) {
      // Text in a child expression: `{busy ? "Saving…" : "Saved"}`.
      for (const l of literalsIn(n.expression)) flag(l, literalText(l)!);
    } else if (ts.isJsxAttribute(n) && TEXT_PROPS.has(n.name.getText(sf))) {
      for (const l of literalsIn(n.initializer)) flag(l, literalText(l)!);
    } else if (ts.isPropertyAssignment(n) && TEXT_FIELDS.has(n.name.getText(sf).replace(/["']/g, ""))) {
      for (const l of literalsIn(n.initializer)) flag(l, literalText(l)!);
    } else if (ts.isBinaryExpression(n) && n.operatorToken.kind === ts.SyntaxKind.EqualsToken && ts.isPropertyAccessExpression(n.left) && DOM_TEXT.has(n.left.name.text)) {
      // DOM text set directly: `el.textContent = "Loading…"`.
      for (const l of literalsIn(n.right)) flag(l, literalText(l)!);
    } else if (ts.isCallExpression(n)) {
      const callee = ts.isPropertyAccessExpression(n.expression) ? n.expression.name.text : ts.isIdentifier(n.expression) ? n.expression.text : "";
      // console.* output is for developers.
      if (ts.isPropertyAccessExpression(n.expression) && n.expression.expression.getText(sf) === "console") return ts.forEachChild(n, visit);
      if (MESSAGE_CALLS.has(callee)) for (const l of literalsIn(n.arguments[0])) flag(l, literalText(l)!);
    }
    const lit = literalText(n);
    // Search keywords list both languages on purpose.
    const keywords = !!n.parent && ts.isPropertyAssignment(n.parent) && n.parent.name.getText(sf) === "keywords";
    if (lit !== null && !keywords && ((GERMAN.test(lit) && !ts.isTemplateExpression(n.parent)) || GERMAN_WORDS.test(lit))) flag(n, lit);
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return hits;
}

describe("no hard-coded UI text", () => {
  it("user-facing strings come from the catalogs", () => {
    const hits = sources(SRC).flatMap(scan);
    const report = hits.map((h) => `${h.file}:${h.line}  ${h.text}`);
    // I18N_REPORT=<file> writes the full list (the assertion diff is cut short).
    if (process.env.I18N_REPORT) fs.writeFileSync(process.env.I18N_REPORT, report.join("\n") + "\n");
    expect(report).toEqual([]);
    // Parsing every source file takes a few seconds while the other test files run too.
  }, 30_000);

  it("the scanner finds the kinds of text it is meant to find", () => {
    const tmp = path.join(SRC, "__scan_probe.tsx");
    fs.writeFileSync(
      tmp,
      [
        'const a = <p title="Speichern">Hello there</p>;',
        'const b = { label: "Open page" };',
        'toast("Saved");',
        'const c = "Größe";',
        'const d = <div className="x" data-id="y">{t("common.save")}</div>;',
        'const e = <span>{busy ? "Saving now" : `${n} left`}</span>;',
        'el.textContent = "Loading now";',
        'const f = { reason: "Broken file", altLabel: "Keep both" };',
        'const g = withHint("Full width", "full_width");',
        'const h = `${n} ${n === 1 ? "Seite" : "Seiten"}`;',
        'const j = <span>{`${n} ${n === 1 ? "Seite" : "Seiten"}`}</span>;',
        'meta.textContent = [n != null ? `${n} ${n === 1 ? "Seite" : "Seiten"}` : ""].join(" ");',
        'const k = { text: done ? "Gebucht" : "" };',
      ].join("\n"),
    );
    try {
      const found = scan(tmp).map((h) => h.text);
      expect(found).toEqual(["Speichern", "Hello there", "Open page", "Saved", "Größe", "Saving now", "{} left", "Loading now", "Broken file", "Keep both", "Full width", "Seite", "Seiten", "Seite", "Seiten", "Seite", "Seiten", "Gebucht"]);
    } finally {
      fs.rmSync(tmp);
    }
  });
});
