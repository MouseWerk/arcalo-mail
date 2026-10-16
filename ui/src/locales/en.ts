// English strings: the default language and the source of the key type (`TKey`), so a key
// missing in de.ts fails the typecheck. Keys are grouped by area (`area.name`). `{name}` is
// filled in by `t(key, { name })`; a `{ one, other }` entry picks its form by the number `n`.
// Rules for both languages: docs/i18n.md (German says „du“).

export type Msg = string | { one: string; other: string };

export const en = {
  // ---- common
  "common.close": "Close",
  "common.back": "Back",
  "common.undo": "Undo",

  // ---- app
  "app.name": "Arcalo Mail",
  "app.demo": "Preview with sample data – opened in the browser without the app",

  // ---- folders and accounts (left pane)
  "nav.label": "Accounts and folders",
  "nav.compose": "New message",
  "nav.noAccount": "No account set up yet.",
  "nav.unread": { one: "{n} unread", other: "{n} unread" },
  "folder.inbox": "Inbox",
  "folder.sent": "Sent",
  "folder.drafts": "Drafts",
  "folder.trash": "Trash",
  "folder.junk": "Spam",
  "folder.archive": "Archive",
  "folder.all": "All mail",
  "folder.flagged": "Flagged",

  // ---- message list
  "list.label": "Messages",
  "list.search": "Search mail…",
  "list.searchLabel": "Search",
  "list.clearSearch": "Clear search",
  "list.empty": "No messages in this folder.",
  "list.noResults": "Nothing found for “{q}”.",
  "list.count": { one: "{n} message", other: "{n} messages" },
  "list.results": { one: "{n} result", other: "{n} results" },
  "list.attachment": "Has attachments",
  "list.unread": "Unread",
  "list.flagged": "Flagged",
  "list.noSubject": "(No subject)",
  "list.unknownSender": "Unknown sender",

  // ---- reading pane
  "read.label": "Reading pane",
  "read.none": "No message selected",
  "read.noneHint": "Choose a message from the list.",
  "read.from": "From",
  "read.to": "To",
  "read.cc": "Cc",
  "read.reply": "Reply",
  "read.replyAll": "Reply all",
  "read.forward": "Forward",
  "read.archive": "Archive",
  "read.delete": "Delete",
  "read.markUnread": "Mark as unread",
  "read.markRead": "Mark as read",
  "read.actions": "Message actions",
  "read.remoteBlocked": {
    one: "One remote image was blocked so the sender cannot see when you read this message.",
    other: "{n} remote images were blocked so the sender cannot see when you read this message.",
  },
  "read.loadRemote": "Load remote content",
  "read.thread": { one: "{n} message in this conversation", other: "{n} messages in this conversation" },
  "read.frameTitle": "Message content",
  "read.loadFailed": "Message not loaded",

  // ---- compose
  "compose.title": "New message",
  "compose.reply": "Reply",
  "compose.forward": "Forward",
  "compose.from": "From",
  "compose.to": "To",
  "compose.cc": "Cc",
  "compose.subject": "Subject",
  "compose.body": "Message",
  "compose.send": "Send",
  "compose.discard": "Discard",
  "compose.sendHint": "Ctrl+Enter sends",
  "compose.sendFailed": "Message not sent",
  "compose.quoteHeader": "On {date}, {name} wrote:",
  "compose.forwardHeader": "Forwarded message",

  // ---- appearance and language
  "theme.label": "Appearance",
  "theme.system": "System",
  "theme.light": "Light",
  "theme.dark": "Dark",
  "theme.contrast": "Contrast",
  "lang.label": "Language",
  "lang.de": "Deutsch",
  "lang.en": "English",

  // ---- dates in the list
  "date.yesterday": "Yesterday",
} satisfies Record<string, Msg>;

export type TKey = keyof typeof en;
