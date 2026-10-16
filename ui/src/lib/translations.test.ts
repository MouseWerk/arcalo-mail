// The translation check (docs/i18n.md), taken from Arcalo: the UI catalogs (`ui/src/locales`),
// the German and English pairs of the backend (`tr!` / `trf!` in crates/ and src-tauri/) and the
// release highlights (docs/releases/highlights, once there are any). It fails on missing or empty texts, placeholders and
// format arguments that differ between the languages, German typography, the wrong form of
// address, terms the glossary (`locales/glossary.json`) forbids, "take … over" for „übernehmen“,
// English words left in German texts and German ones in English texts, and common German
// spelling slips. German texts much longer than their English button or menu label are only
// reported (console warning).
// True exceptions are listed in ALLOWED with the reason.

import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { en, type Msg } from "../locales/en";
import { de } from "../locales/de";

const ROOT = path.resolve(__dirname, "../../..");
type Lang = "de" | "en";
type Text = { where: string; lang: Lang; text: string };
type Finding = { rule: string; where: string; text: string; detail: string };

// ---- exceptions

/** A finding of `rule` in a text at `where` (a catalog key, a Rust file, a highlights file) that
 *  contains `text` is accepted. */
const ALLOWED: { rule: string; where: string; text?: string; reason: string }[] = [];

/** Texts with „{n} …s“ that need no { one, other } plural because the count is never 1. */
const COUNT_NEVER_ONE: Record<string, string> = {};

/** Format arguments with a different name per language (a constant with an `_EN` twin). */
const RENAMED_ARGS: Record<string, string> = {};

/** Accepted English words in German texts (product terms and established loanwords). The check
 *  below only looks for the words in ENGLISH_IN_DE, which must not contain any of these. */
const ANGLICISMS = [
  "Issue", "Issues", "Canvas", "Briefing", "Sprint", "Widget", "Widgets", "Tab", "Tabs", "Link", "Links", "Tag", "Tags",
  "Git", "PDF", "KI", "Board", "Chat", "Token", "Tokens", "Timer", "Update", "Updates", "Download", "Upload", "Server",
  "Proxy", "Commit", "Branch", "Remote", "Repository", "Embedding", "Embeddings", "Reasoning", "Cloud", "Site", "Sites",
  "Worklog", "Workshop", "Kanban", "Callout", "Checkbox", "Heatmap", "Burndown", "Countdown", "Pomodoro", "Vault",
  "Release", "Feed", "Mindmap", "Gateway", "Deployment", "Deployments", "Installer", "Review", "Daily", "Status", "Code",
  "Cc", "Bcc", "Spam", "Port", "IMAP", "SMTP", "JMAP", "OAuth", "TLS", "STARTTLS", "App",
];
/** English UI words that must not be left in German texts. */
const ENGLISH_IN_DE = [
  "Settings", "Save", "Cancel", "Loading", "Error", "Errors", "Open", "Delete", "Edit", "Close", "Search", "Remove",
  "Undo", "Redo", "Copy", "Paste", "Done", "Failed", "Saved", "Please", "Click", "Select", "Show", "Hide", "Back",
  "Next", "Previous", "Enable", "Disable", "Enabled", "Disabled", "Warning", "Success", "Preview", "Untitled",
  "Summary", "Timeline", "Network", "Today", "Tomorrow", "Yesterday", "Week", "Month", "Year", "Page", "Pages",
  "Note", "Notes", "Folder", "Folders", "File", "Files", "New", "Reset", "Apply", "Add", "Booking", "Bookings",
  "the", "and", "with", "for", "not", "your", "you", "this", "that", "of", "is", "are",
];
/** German words that must not be left in English texts (besides umlauts, ß and „). */
const GERMAN_IN_EN = [
  "und", "oder", "nicht", "der", "die", "das", "mit", "für", "bitte", "wird", "werden", "Seite", "Seiten",
  "Einstellungen", "Aufgabe", "Aufgaben", "Notiz", "Notizen", "Datei", "Ordner", "heute", "gestern", "Termin",
  "Termine", "Buchung", "Sicherung", "Besprechung",
];
/** ae/oe/ue/ss written where German needs ä/ö/ü/ß: word stems that never occur in correct German
 *  (also inside compounds), and short words that must stand alone. */
const TRANSLITERATIONS = new RegExp(
  [
    String.raw`\p{L}*(schluessel|loesch|groess|oeffn|aender|uebernehm|faellig|moeglich|unterstuetz|ausfuehr|zurueck|naechst|spaeter|muess|koenn|moecht|waehl|haeufig|schliess|gruess|massnahm)\p{L}*`,
    String.raw`(?<!\p{L})(fuer|ueber|gross|grosse[nrms]?|weiss|heisst|strasse|fuss|ausserdem)(?!\p{L})`,
  ].join("|"),
  "giu",
);
/** Common nouns written in lower case right after an article. */
const LOWERCASE_NOUN =
  /\b(der|die|das|den|dem|des|ein|eine|einer|einen|einem|eines) (seite|seiten|notiz|notizen|aufgabe|aufgaben|datei|dateien|einstellungen|buchung|buchungen|sicherung|termin|termine|besprechung|vorlage|ordner|zeiterfassung|tagesnotiz)\b/g;

// ---- sources

function textsOf(lang: Lang, cat: Record<string, Msg>): Text[] {
  return Object.entries(cat).flatMap(([key, msg]) =>
    typeof msg === "string"
      ? [{ where: key, lang, text: msg }]
      : [{ where: `${key}.one`, lang, text: msg.one }, { where: `${key}.other`, lang, text: msg.other }],
  );
}

function rsFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "target" ? [] : rsFiles(p);
    return e.name.endsWith(".rs") ? [p] : [];
  });
}

/** The value of a Rust string literal (`"…"` with escapes and line continuations, or `r#"…"#`). */
export function rustString(lit: string): string {
  const raw = lit.match(/^r(#*)"([\s\S]*)"\1$/);
  if (raw) return raw[2];
  return lit
    .slice(1, -1)
    .replace(/\\\r?\n\s*/g, "")
    .replace(/\\(u\{[0-9a-fA-F]+\}|.)/g, (_, c: string) =>
      c.startsWith("u{") ? String.fromCodePoint(parseInt(c.slice(2, -1), 16)) : ({ n: "\n", t: "\t", r: "\r", "0": "\0" } as Record<string, string>)[c] ?? c,
    );
}

const LIT = String.raw`(?:r(#*)"[\s\S]*?"\1|"(?:[^"\\]|\\[\s\S])*")`;
const TR_CALL = new RegExp(String.raw`\b(trf?)!\(\s*(` + LIT + String.raw`)\s*,\s*(` + LIT + String.raw`)`, "g");

export type RustPair = { where: string; file: string; format: boolean; de: string; en: string };

/** Every `tr!` / `trf!` call with two string literals in the backend sources. */
function rustPairs(): RustPair[] {
  const files = [...rsFiles(path.join(ROOT, "crates")), ...rsFiles(path.join(ROOT, "src-tauri/src"))];
  return files.flatMap((f) => {
    const src = fs.readFileSync(f, "utf8");
    const file = path.relative(ROOT, f).split(path.sep).join("/");
    return [...src.matchAll(TR_CALL)].map((m) => ({
      where: `${file}:${src.slice(0, m.index).split("\n").length}`,
      file,
      format: m[1] === "trf",
      de: rustString(m[2]),
      en: rustString(m[4]),
    }));
  });
}

/** The arguments of a `format!` string: names, and `#n` for the n-th positional one. */
export function formatArgs(s: string): string[] {
  let pos = 0;
  return [...s.replace(/\{\{|\}\}/g, "").matchAll(/\{([^{}]*)\}/g)]
    .map((m) => m[1].split(":")[0].trim())
    .map((name) => (name === "" || /^\d+$/.test(name) ? `#${name === "" ? pos++ : name}` : name))
    .sort();
}

/** The `{name}` placeholders of a catalog text. */
export function placeholders(s: string): string[] {
  return [...s.matchAll(/\{([A-Za-z_]\w*)\}/g)].map((m) => m[1]).sort();
}

function highlightTexts(): Text[] {
  const dir = path.join(ROOT, "docs/releases/highlights");
  if (!fs.existsSync(dir)) return [];
  return fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .flatMap((f) => {
      const doc = JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")) as { items: Record<string, unknown>[] };
      return doc.items.flatMap((item) =>
        (["de", "en"] as const).flatMap((lang) =>
          Object.entries((item[lang] ?? {}) as Record<string, string>)
            .filter(([field]) => field !== "image")
            .map(([field, text]) => ({ where: `docs/releases/highlights/${f}#${String(item.id)}.${field}`, lang, text })),
        ),
      );
    });
}

// ---- rules

/** Text without code: `inline code`, [[links]], URLs and {placeholders} become a neutral word, so
 *  the rules see the sentence around them. */
function prose(s: string): string {
  return s
    .replace(/```[\s\S]*?```/g, "x0")
    .replace(/`[^`\n]*`/g, "x0")
    .replace(/\{\{?[\w.:+-]*\}\}?/g, "x0")
    .replace(/\[\[[^\]]*\]\]/g, "x0")
    .replace(/(https?|file|webcal):\/\/[^\s,;)]*/g, "x0");
}

const at = (t: Text, rule: string, detail: string): Finding => ({ rule, where: t.where, text: t.text, detail });

/** German quotes „…“, no straight or English quotes in German; no German quotes in English. */
function quotes(t: Text): Finding[] {
  const s = prose(t.text);
  const out: Finding[] = [];
  for (const m of s.matchAll(/"([^"\n]*)"/g)) {
    // Code-like content (identifiers, syntax) may stay in straight quotes.
    if (!/[/=(){}[\]<>_*#@:\\]|^[a-z0-9.\- ]*$/.test(m[1]) || /^[A-ZÄÖÜ]/.test(m[1])) out.push(at(t, "quotes", m[0]));
  }
  if (t.lang === "de") {
    let open = 0;
    for (const c of s) {
      if (c === "„") open++;
      else if (c === "“") {
        if (open > 0) open--;
        else out.push(at(t, "quotes", "“ opens a quote (German: „…“)"));
      } else if (c === "”") out.push(at(t, "quotes", "” (German closes with “)"));
    }
    if (open > 0) out.push(at(t, "quotes", "„ without closing “"));
  } else if (s.includes("„")) out.push(at(t, "quotes", "German quote „ in English"));
  return out;
}

function typography(t: Text): Finding[] {
  const s = prose(t.text);
  const out: Finding[] = [];
  if (s.includes("...")) out.push(at(t, "ellipsis", "... (use …)"));
  // A label, placeholder or progress text ends in „…“ right after the word („Speichern…“); a
  // space before „…“ only where words are left out inside a sentence.
  if (/[\p{L}\p{N})] …$/u.test(s.trim())) out.push(at(t, "ellipsis", "space before the final … („Speichern…“)"));
  for (const m of s.matchAll(/[^\s] {2,}[^\s]/g)) out.push(at(t, "double-space", JSON.stringify(m[0])));
  for (const m of s.matchAll(/[^\s] +([,;!?]|[.:](?=\s|$))(?=\s|$|["“”)])/g)) out.push(at(t, "space-before-punct", JSON.stringify(m[0])));
  for (const m of s.matchAll(/[\p{L}\p{N})“”"] - [\p{L}\p{N}„"(]/gu)) out.push(at(t, "dash", `${JSON.stringify(m[0])} (use –)`));
  if (t.lang === "de") {
    for (const m of s.matchAll(/\b(Min|Std|Sek)\b(?!\.)|(?<![\w./-])(min|std)\b(?!\.)/g)) out.push(at(t, "units", `${m[0]} (Min., Std.)`));
    for (const m of s.matchAll(/\d(%|MB|GB|KB|px|ms)\b/g)) out.push(at(t, "units", `${m[0]} (space between number and unit)`));
    for (const m of s.matchAll(/(?<![\d/])\d{1,2}\/\d{1,2}\/\d{4}(?![\d/])|(?<![\w@:.-])\d{4}-\d\d-\d\d\b|\b(\d\.\d{1,2}|\d{1,2}\.\d)\.\d{4}\b/g))
      out.push(at(t, "dates", `${m[0]} (German dates: 02.10.2026)`));
  }
  return out;
}

/** Arcalo Mail says „du“: no „Sie“, „Ihr …“, „Ihnen“ addressing the reader. A „Sie“ or „Ihr …“ at the
 *  start of a sentence is mostly „they“ / „its“ and only counts in the formal imperative. */
function address(t: Text): Finding[] {
  if (t.lang !== "de") return [];
  const s = prose(t.text);
  const out: Finding[] = [];
  for (const m of s.matchAll(/\b(Sie|Ihnen|Ihre?[mnrs]?)\b(?!-)/g)) {
    const before = s.slice(0, m.index);
    const sentenceStart = /(^|[.!?:;„(\n–]\s*)$/.test(before);
    if (!sentenceStart || m[1] === "Ihnen") out.push(at(t, "address", `„${m[1]}“ (Arcalo Mail says „du“)`));
  }
  for (const m of s.matchAll(/(^|[.!?:]\s+)([A-ZÄÖÜ][a-zäöüß]+(en|ern|eln)) Sie\b/g)) out.push(at(t, "address", `„${m[2]} Sie“ (Arcalo Mail says „du“)`));
  return out;
}

type Term = { concept: string; de: string; en: string; forbiddenDe: string[]; forbiddenEn: string[]; note?: string };
const GLOSSARY = JSON.parse(fs.readFileSync(path.join(__dirname, "../locales/glossary.json"), "utf8")) as { terms: Term[] };

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
/** A word (or, with a trailing *, a word start) at a word boundary. */
function wordRe(variant: string): RegExp {
  const prefix = variant.endsWith("*");
  const w = escape(prefix ? variant.slice(0, -1) : variant);
  return new RegExp(`(?<![\\p{L}\\p{N}_#@/.-])${w}${prefix ? "" : "(?![\\p{L}\\p{N}_])"}`, "gu");
}
const FORBIDDEN = GLOSSARY.terms.flatMap((term) => [
  ...term.forbiddenDe.map((v) => ({ lang: "de" as Lang, re: wordRe(v), variant: v, term })),
  ...term.forbiddenEn.map((v) => ({ lang: "en" as Lang, re: wordRe(v), variant: v, term })),
]);

function glossary(t: Text): Finding[] {
  const s = prose(t.text);
  return FORBIDDEN.filter((f) => f.lang === t.lang && f.re.test(s) && ((f.re.lastIndex = 0), true)).map((f) =>
    at(t, "glossary", `„${f.variant.replace(/\*$/, "")}“ – use „${t.lang === "de" ? f.term.de : f.term.en}“ (${f.term.concept})`),
  );
}

const ENGLISH_RE = ENGLISH_IN_DE.map((w) => ({ w, re: wordRe(w) }));
const GERMAN_RE = GERMAN_IN_EN.map((w) => ({ w, re: wordRe(w) }));

function leftovers(t: Text): Finding[] {
  const s = prose(t.text);
  if (t.lang === "de") return ENGLISH_RE.filter(({ re }) => ((re.lastIndex = 0), re.test(s))).map(({ w }) => at(t, "english", `„${w}“ in German`));
  const out = GERMAN_RE.filter(({ re }) => ((re.lastIndex = 0), re.test(s))).map(({ w }) => at(t, "german", `„${w}“ in English`));
  const umlaut = s.match(/[äöüÄÖÜß]/);
  if (umlaut) out.push(at(t, "german", `„${umlaut[0]}“ in English`));
  return out;
}

function spelling(t: Text): Finding[] {
  if (t.lang !== "de") return [];
  const s = prose(t.text);
  return [
    ...[...s.matchAll(TRANSLITERATIONS)].map((m) => at(t, "transliteration", `${m[0]} (ä/ö/ü/ß)`)),
    // „die die App anlegt“ (relative pronoun and article) is correct German, so articles may repeat.
    ...[...s.matchAll(/(?<![\p{L}])(\p{L}{2,})\s+\1(?![\p{L}])/giu)]
      .filter((m) => !/^(der|die|das|dem|den)$/i.test(m[1]))
      .map((m) => at(t, "doubled-word", m[0])),
    ...[...s.matchAll(LOWERCASE_NOUN)].map((m) => at(t, "lowercase-noun", m[0])),
  ];
}

/** Error titles (keys ending in „Failed“) say what failed: „Seite nicht gespeichert“ / "Page not
 *  saved", or the action: „Speichern fehlgeschlagen“. Not „Nicht gespeichert“, „Konnte nicht …“,
 *  "Could not …" or a bare „Fehler“; the cause goes in the detail (docs/i18n.md). */
function errorTitle(t: Text): Finding[] {
  if (!/Failed(\.one|\.other)?$/.test(t.where)) return [];
  const bad = t.lang === "de" ? /^(Nicht |Konnte nicht|Konnten nicht|Fehler$|Fehlgeschlagen$)/ : /^(Not |Could not|Error$|Failed$)/;
  return bad.test(t.text.trim()) ? [at(t, "error-title", "name what failed: „<Ding> nicht <Partizip>“ / \"<Thing> not <participle>\"")] : [];
}

/** „übernehmen“ is not "take over", also not split around its object ("Take all over", "takes
 *  them over"): e-mails and files are imported, settings and changes applied, data kept. The
 *  glossary only finds the words next to each other. */
function takeOver(t: Text): Finding[] {
  if (t.lang !== "en") return [];
  const m = prose(t.text).match(/(?<![\p{L}])(take|takes|taking|took|taken)(\s+[\p{L}\p{N}’']+){0,3}\s+over(?![\p{L}])/iu);
  return m ? [at(t, "take-over", `„${m[0]}“ – use „import“, „apply“ or „keep“`)] : [];
}

const RULES = [quotes, typography, address, glossary, leftovers, spelling, errorTitle, takeOver];

/** Whether the exception `a` covers the finding `f` (a key covers its plural forms, a file all
 *  its lines). */
function matches(a: (typeof ALLOWED)[number], f: Finding): boolean {
  const w = f.where;
  return (
    a.rule === f.rule &&
    (w === a.where || [".", ":", "#"].some((sep) => w.startsWith(a.where + sep))) &&
    (a.text === undefined || f.text.includes(a.text))
  );
}
const allowed = (f: Finding) => ALLOWED.some((a) => matches(a, f));

function report(findings: Finding[]): string {
  return findings.map((f) => `${f.rule} · ${f.where}: ${f.detail}\n    ${f.text.slice(0, 160)}`).join("\n");
}

// ---- tests

const PAIRS = rustPairs();
const TEXTS: Text[] = [
  ...textsOf("de", de),
  ...textsOf("en", en),
  ...PAIRS.flatMap((p) => [
    { where: p.where, lang: "de" as Lang, text: p.de },
    { where: p.where, lang: "en" as Lang, text: p.en },
  ]),
  ...highlightTexts(),
];

describe("translations", () => {
  it("has every key in both catalogs, no empty text and the same plural forms", () => {
    const enKeys = Object.keys(en);
    const deKeys = Object.keys(de);
    expect(deKeys.filter((k) => !(k in en)), "only in de.ts").toEqual([]);
    expect(enKeys.filter((k) => !(k in de)), "only in en.ts").toEqual([]);
    const bad: string[] = [];
    for (const k of enKeys) {
      const a = en[k as keyof typeof en] as Msg;
      const b = de[k as keyof typeof de] as Msg;
      if (typeof a !== typeof b) bad.push(`${k}: plural in only one catalog`);
      for (const [lang, m] of [["en", a], ["de", b]] as const) {
        const forms = typeof m === "string" ? [m] : [m.one, m.other];
        if (forms.some((f) => typeof f !== "string" || f.trim() === "")) bad.push(`${k} (${lang}): empty`);
      }
    }
    expect(bad).toEqual([]);
  });

  it("uses the same placeholders in German and English, and plural forms where a count matters", () => {
    const bad: string[] = [];
    for (const k of Object.keys(en)) {
      const a = en[k as keyof typeof en] as Msg;
      const b = de[k as keyof typeof de] as Msg;
      const forms: [string, string, string][] =
        typeof a === "string" || typeof b === "string"
          ? [[k, typeof a === "string" ? a : a.other, typeof b === "string" ? b : b.other]]
          : [[`${k}.one`, a.one, b.one], [`${k}.other`, a.other, b.other]];
      for (const [where, x, y] of forms) {
        const px = placeholders(x).join(",");
        const py = placeholders(y).join(",");
        if (px !== py) bad.push(`${where}: en {${px}} – de {${py}}`);
      }
      if (typeof a === "string" && typeof b === "string" && /\{\w+\}/.test(a)) {
        const hack = `${a} ${b}`.match(/\p{L}\((s|e|en|n)\)/u);
        if (hack) bad.push(`${k}: „${hack[0]}“ – use a { one, other } plural`);
        // Any count placeholder, not only {n}: „{pages} Seiten“ read „1 Seiten“.
        const counted = a.match(/\{(n|count|total|pages|links|files|days|hours|\w+Count)\} (?!ms\b)([a-z]+s)\b/);
        if (counted && !(k in COUNT_NEVER_ONE)) bad.push(`${k}: „{${counted[1]}} ${counted[2]}“ without a { one, other } plural`);
      }
    }
    expect(bad, bad.join("\n")).toEqual([]);
  });

  it("parses the tr!/trf! pairs of the backend", () => {
    expect(PAIRS.length).toBeGreaterThan(10);
    expect(PAIRS.filter((p) => p.de.trim() === "" || p.en.trim() === "").map((p) => p.where)).toEqual([]);
  });

  it("gives trf! the same format arguments in German and English", () => {
    const bad = formatFindings().filter((f) => f.detail !== "" && !allowed(f));
    expect(bad, report(bad)).toEqual([]);
  });

  for (const rule of RULES) {
    it(`passes the ${rule.name} rules`, () => {
      const bad = TEXTS.flatMap(rule).filter((f) => !allowed(f));
      expect(bad, report(bad)).toEqual([]);
    });
  }

  it("keeps the glossary and the word lists consistent", () => {
    expect(ENGLISH_IN_DE.filter((w) => ANGLICISMS.includes(w))).toEqual([]);
    for (const term of GLOSSARY.terms) {
      expect(term.de && term.en, term.concept).toBeTruthy();
      // A preferred term is never one of its own forbidden variants.
      for (const v of term.forbiddenDe) expect(wordRe(v).test(term.de.split(" (")[0]), `${term.concept}: ${v}`).toBe(false);
      for (const v of term.forbiddenEn) expect(wordRe(v).test(term.en), `${term.concept}: ${v}`).toBe(false);
    }
  });

  it("only lists exceptions that are still needed", () => {
    const all = [...TEXTS.flatMap((t) => RULES.flatMap((r) => r(t))), ...formatFindings().filter((f) => f.detail !== "")];
    const unused = ALLOWED.filter((a) => !all.some((f) => matches(a, f)));
    expect(unused.map((a) => `${a.rule} · ${a.where}`)).toEqual([]);
  });

  it("reports German button and menu labels much longer than the English ones", () => {
    const long = Object.keys(en).flatMap((k) => {
      const a = en[k as keyof typeof en] as Msg;
      const b = de[k as keyof typeof de] as Msg;
      if (typeof a !== "string" || typeof b !== "string") return [];
      const label = a.length <= 30 && !/[.:!?]\s|[.!?]$/.test(a);
      return label && b.length > 30 && b.length > 2 * a.length ? [`${k}: „${b}“ (${b.length}) – “${a}” (${a.length})`] : [];
    });
    // A warning, not a failure: long labels are worth a look but may be right.
    if (long.length) console.warn(`German labels more than twice as long as the English ones:\n${long.join("\n")}`);
  });
});

describe("translation rules", () => {
  const de1 = (text: string): Text => ({ where: "x", lang: "de", text });
  const en1 = (text: string): Text => ({ where: "x", lang: "en", text });
  const rules = (t: Text) => RULES.flatMap((r) => r(t)).map((f) => f.rule);

  it("finds the mistakes they are meant for", () => {
    expect(rules(de1('Datei "Notiz" öffnen'))).toContain("quotes");
    expect(rules(de1("Datei “Notiz” öffnen"))).toContain("quotes");
    expect(rules(de1("Lädt..."))).toContain("ellipsis");
    expect(rules(de1("Wird synchronisiert …"))).toContain("ellipsis");
    expect(rules(en1("Import bookmarks …"))).toContain("ellipsis");
    expect(rules(de1("Strg+Umschalt+K drücken"))).toEqual([]);
    expect(rules(de1("Ctrl+Shift+K drücken"))).toContain("glossary");
    expect(rules(de1("Enter speichert"))).toContain("glossary");
    expect(rules(en1("Nothing was taken over"))).toContain("glossary");
    expect(rules(en1("Nothing was taken over"))).toContain("take-over");
    expect(rules(en1("Take all over"))).toContain("take-over");
    expect(rules(en1("Take {n} over"))).toContain("take-over");
    expect(rules(en1("“Book again” on the chip takes them over."))).toContain("take-over");
    expect(rules(en1("Apply all · Apply {n} · Import e-mail"))).toEqual([]);
    expect(rules(en1("Open the email"))).toContain("glossary");
    expect(rules({ where: "x.saveFailed", lang: "de", text: "Nicht gespeichert" })).toContain("error-title");
    expect(rules({ where: "x.saveFailed", lang: "en", text: "Could not save" })).toContain("error-title");
    expect(rules({ where: "x.saveFailed", lang: "en", text: "Page not saved" })).toEqual([]);
    expect(rules(de1("Zwei  Leerzeichen"))).toContain("double-space");
    expect(rules(de1("Fertig !"))).toContain("space-before-punct");
    expect(rules(de1("Projekt - Netzplan"))).toContain("dash");
    expect(rules(de1("in 10 Min"))).toContain("units");
    expect(rules(de1("am 10/02/2026"))).toContain("dates");
    expect(rules(de1("Bitte geben Sie die PIN ein."))).toContain("address");
    expect(rules(de1("Geben Sie die PIN ein."))).toContain("address");
    expect(rules(de1("Ihre Notizen bleiben lokal, wenn Ihnen das lieber ist."))).toContain("address");
    expect(rules(de1("Neuen Account anlegen"))).toContain("glossary");
    expect(rules(de1("Thread öffnen"))).toContain("glossary");
    expect(rules(de1("Neue Email"))).toContain("glossary");
    expect(rules(de1("Seite Save"))).toContain("english");
    expect(rules(en1("Open the Seite"))).toContain("german");
    expect(rules(en1("Geöffnet"))).toContain("german");
    expect(rules(de1("Datei loeschen"))).toContain("transliteration");
    expect(rules(de1("Wiederherstellungsschluessel"))).toContain("transliteration");
    expect(rules(de1("Die Seite Seite ist leer"))).toContain("doubled-word");
    expect(rules(de1("Öffnet die seite"))).toContain("lowercase-noun");
  });

  it("leaves correct texts alone", () => {
    for (const ok of [
      "„Angebot“ öffnen – mit `\"code\"` und [[Seite \"A\"]]",
      "Sie werden bei jeder Synchronisierung neu gelesen.",
      "Formuliere den Text förmlicher (Sie-Form).",
      "Jede Art von Nachricht, die die App anlegt",
      "Warnt ab 80 %, blockiert bei 100 % · 10 Min. · 2 Std.",
      "z. B. am 02.10.2026 um 10 Uhr",
      "Externe Inhalte laden, Anhang speichern, Cc und Spam",
      "„Was steht in der Nachricht vom …?“ und Senden…",
    ])
      expect(rules(de1(ok)), ok).toEqual([]);
    expect(rules(en1("Open the “Sent” folder – 3 e-mails, 10 min"))).toEqual([]);
  });

  it("reads Rust string literals and format arguments", () => {
    expect(rustString('"a\\n\\"b\\" \\\n      c"')).toBe('a\n"b" c');
    expect(rustString('r#"x "y" z"#')).toBe('x "y" z');
    expect(formatArgs("{} von {:.1} h ({name}, {{literal}}, {d:+})")).toEqual(["#0", "#1", "d", "name"]);
    expect(placeholders("{n} Seiten in „{title}“")).toEqual(["n", "title"]);
  });
});

function formatFindings(): Finding[] {
  return PAIRS.filter((p) => p.format).map((p) => {
    const d = formatArgs(p.de).map((a) => RENAMED_ARGS[a] ?? a).sort().join(",");
    const e = formatArgs(p.en).join(",");
    return { rule: "format-args", where: p.where, text: p.de, detail: d === e ? "" : `de {${d}} – en {${e}}` };
  });
}
