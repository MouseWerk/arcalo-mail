// German strings: the same keys as en.ts (the typecheck fails on a missing one). German says
// „du“, uses „…“ quotes and the glossary's terms (glossary.json, docs/i18n.md).

import type { Msg, TKey } from "./en";

export const de: Record<TKey, Msg> = {
  // ---- common
  "common.close": "Schließen",
  "common.back": "Zurück",
  "common.undo": "Rückgängig",

  // ---- app
  "app.name": "Arcalo Mail",
  "app.demo": "Vorschau mit Beispieldaten – im Browser ohne die App geöffnet",

  // ---- folders and accounts (left pane)
  "nav.label": "Konten und Ordner",
  "nav.compose": "Neue Nachricht",
  "nav.noAccount": "Noch kein Konto eingerichtet.",
  "nav.unread": { one: "{n} ungelesen", other: "{n} ungelesen" },
  "folder.inbox": "Posteingang",
  "folder.sent": "Gesendet",
  "folder.drafts": "Entwürfe",
  "folder.trash": "Papierkorb",
  "folder.junk": "Spam",
  "folder.archive": "Archiv",
  "folder.all": "Alle Nachrichten",
  "folder.flagged": "Markiert",

  // ---- message list
  "list.label": "Nachrichten",
  "list.search": "E-Mails durchsuchen…",
  "list.searchLabel": "Suchen",
  "list.clearSearch": "Suche leeren",
  "list.empty": "Keine Nachrichten in diesem Ordner.",
  "list.noResults": "Nichts gefunden für „{q}“.",
  "list.count": { one: "{n} Nachricht", other: "{n} Nachrichten" },
  "list.results": { one: "{n} Treffer", other: "{n} Treffer" },
  "list.attachment": "Mit Anhang",
  "list.unread": "Ungelesen",
  "list.flagged": "Markiert",
  "list.noSubject": "(Kein Betreff)",
  "list.unknownSender": "Unbekannter Absender",

  // ---- reading pane
  "read.label": "Lesebereich",
  "read.none": "Keine Nachricht ausgewählt",
  "read.noneHint": "Wähle eine Nachricht in der Liste.",
  "read.from": "Von",
  "read.to": "An",
  "read.cc": "Cc",
  "read.reply": "Antworten",
  "read.replyAll": "Allen antworten",
  "read.forward": "Weiterleiten",
  "read.archive": "Archivieren",
  "read.delete": "Löschen",
  "read.markUnread": "Als ungelesen markieren",
  "read.markRead": "Als gelesen markieren",
  "read.actions": "Aktionen für die Nachricht",
  "read.remoteBlocked": {
    one: "Ein externes Bild wurde blockiert, damit der Absender nicht sieht, wann du die Nachricht liest.",
    other: "{n} externe Bilder wurden blockiert, damit der Absender nicht sieht, wann du die Nachricht liest.",
  },
  "read.loadRemote": "Externe Inhalte laden",
  "read.thread": { one: "{n} Nachricht in dieser Unterhaltung", other: "{n} Nachrichten in dieser Unterhaltung" },
  "read.frameTitle": "Inhalt der Nachricht",
  "read.loadFailed": "Nachricht nicht geladen",

  // ---- compose
  "compose.title": "Neue Nachricht",
  "compose.reply": "Antworten",
  "compose.forward": "Weiterleiten",
  "compose.from": "Von",
  "compose.to": "An",
  "compose.cc": "Cc",
  "compose.subject": "Betreff",
  "compose.body": "Nachricht",
  "compose.send": "Senden",
  "compose.discard": "Verwerfen",
  "compose.sendHint": "Strg+Eingabe sendet",
  "compose.sendFailed": "Nachricht nicht gesendet",
  "compose.quoteHeader": "Am {date} schrieb {name}:",
  "compose.forwardHeader": "Weitergeleitete Nachricht",

  // ---- appearance and language
  "theme.label": "Darstellung",
  "theme.system": "Wie das System",
  "theme.light": "Hell",
  "theme.dark": "Dunkel",
  "theme.contrast": "Kontrast",
  "lang.label": "Sprache",
  "lang.de": "Deutsch",
  "lang.en": "English",

  // ---- dates in the list
  "date.yesterday": "Gestern",
};
