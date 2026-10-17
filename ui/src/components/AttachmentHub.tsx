// „Anhänge“: every attachment of the mail on this device in one list, newest first, filtered by
// kind, sender, time and name; each with its preview, „Speichern“ and the way to its message.

import { Download, Eye, File, FileArchive, FileImage, FileSpreadsheet, FileText, CalendarDays, Mail, Menu as MenuIcon, Paperclip, Presentation, Search, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { displayName, fileSize, listDate, now } from "../lib/format";
import type { FileKind, HubFilter, HubItem, HubSender } from "../lib/types";
import { Spinner } from "./ui";

export const KINDS: [FileKind, TKey][] = [
  ["images", "hub.images"],
  ["pdf", "hub.pdf"],
  ["documents", "hub.documents"],
  ["spreadsheets", "hub.spreadsheets"],
  ["presentations", "hub.presentations"],
  ["archives", "hub.archives"],
  ["calendar", "hub.calendar"],
  ["other", "hub.other"],
];

export function kindIcon(k: FileKind) {
  return { images: FileImage, pdf: FileText, documents: FileText, spreadsheets: FileSpreadsheet, presentations: Presentation, archives: FileArchive, calendar: CalendarDays, other: File }[k];
}

type Range = "all" | "week" | "month" | "year";

/** Previewable in the app: pictures (not SVG) and plain text. */
export const previewable = (mime: string) => (mime.startsWith("image/") && !mime.includes("svg")) || mime.startsWith("text/plain") || mime === "text/csv";

export function AttachmentHub({
  accountId,
  revision,
  onPreview,
  onSave,
  onOpenFile,
  onOpenMessage,
  onShowFolders,
}: {
  accountId: number | null;
  /** Bumped when mail changed: the list loads again. */
  revision: number;
  onPreview: (it: HubItem) => void;
  onSave: (it: HubItem) => void;
  onOpenFile: (it: HubItem) => void;
  onOpenMessage: (it: HubItem) => void;
  onShowFolders: () => void;
}) {
  const t = useT();
  const lang = currentLang();
  const [kind, setKind] = useState<FileKind | null>(null);
  const [sender, setSender] = useState<string | null>(null);
  const [range, setRange] = useState<Range>("all");
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<HubItem[] | null>(null);
  const [total, setTotal] = useState(0);
  const [senders, setSenders] = useState<HubSender[]>([]);
  const filter = useMemo<HubFilter>(() => {
    const day = 86400;
    const since = range === "all" ? null : Math.floor(Date.now() / 1000) - (range === "week" ? 7 : range === "month" ? 31 : 366) * day;
    return { kind, sender, since, until: null, query, account_id: accountId };
  }, [kind, sender, range, query, accountId]);
  useEffect(() => {
    let alive = true;
    const timer = setTimeout(() => {
      void api.hub(filter, 0, 300).then((p) => {
        if (!alive) return;
        setItems(p.items);
        setTotal(p.total);
      });
    }, 120);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [filter, revision]);
  useEffect(() => {
    void api.hubSenders(accountId).then(setSenders);
  }, [accountId, revision]);
  const filtered = kind || sender || range !== "all" || query.trim();
  return (
    <section className="list-pane smart-pane hub-pane" aria-label={t("hub.title")}>
      <header className="list-head">
        <div className="list-title">
          <button type="button" className="icon-btn folders-btn" onClick={onShowFolders} aria-label={t("nav.label")}>
            <MenuIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
          <h1>{t("hub.title")}</h1>
          {items && <span className="muted list-count">{total}</span>}
        </div>
        <div className="search hub-search">
          <Search size={14} strokeWidth={1.75} aria-hidden className="search-icon" />
          <input className="search-input" value={query} placeholder={t("hub.search")} aria-label={t("hub.search")} onChange={(e) => setQuery(e.target.value)} />
          {query && (
            <button type="button" className="icon-btn icon-btn-xs search-clear" onClick={() => setQuery("")} aria-label={t("common.close")}>
              <X size={12} strokeWidth={2} aria-hidden />
            </button>
          )}
        </div>
        <div className="hub-filters" role="group" aria-label={t("hub.filters")}>
          <select className="select select-sm" value={kind ?? ""} onChange={(e) => setKind((e.target.value || null) as FileKind | null)} aria-label={t("hub.kind")}>
            <option value="">{t("hub.allKinds")}</option>
            {KINDS.map(([k, label]) => (
              <option key={k} value={k}>
                {t(label)}
              </option>
            ))}
          </select>
          <select className="select select-sm" value={sender ?? ""} onChange={(e) => setSender(e.target.value || null)} aria-label={t("hub.sender")}>
            <option value="">{t("hub.allSenders")}</option>
            {senders.map((s) => (
              <option key={s.email} value={s.email}>
                {s.name ? `${s.name} (${s.files})` : `${s.email} (${s.files})`}
              </option>
            ))}
          </select>
          <select className="select select-sm" value={range} onChange={(e) => setRange(e.target.value as Range)} aria-label={t("hub.time")}>
            <option value="all">{t("hub.anyTime")}</option>
            <option value="week">{t("hub.week")}</option>
            <option value="month">{t("hub.month")}</option>
            <option value="year">{t("hub.year")}</option>
          </select>
        </div>
      </header>
      {items === null ? (
        <div className="empty smart-empty">
          <Spinner label={t("hub.loading")} />
        </div>
      ) : items.length === 0 ? (
        <div className="empty smart-empty">
          <Paperclip size={28} strokeWidth={1.5} aria-hidden />
          <p>{filtered ? t("hub.noneFiltered") : t("hub.none")}</p>
        </div>
      ) : (
        <ul className="hub-list" aria-label={t("hub.title")}>
          {items.map((it) => {
            const Icon = kindIcon(it.kind);
            const name = it.filename ?? t("read.unnamed");
            const canPreview = previewable(it.mime_type);
            return (
              <li key={it.id} className="hub-item">
                <button type="button" className="hub-main" onClick={() => (canPreview ? onPreview(it) : onOpenFile(it))} title={canPreview ? t("read.preview") : t("read.open")}>
                  <span className="attachment-icon" data-kind={it.kind}>
                    <Icon size={18} strokeWidth={1.5} aria-hidden />
                  </span>
                  <span className="hub-text">
                    <span className="hub-name">{name}</span>
                    <span className="hub-meta muted">
                      <span>{fileSize(it.size, lang)}</span>
                      <span aria-hidden>·</span>
                      <span className="hub-from">{displayName(it.from, lang)}</span>
                      <span aria-hidden>·</span>
                      <span>{listDate(it.date, now(), lang)}</span>
                    </span>
                  </span>
                </button>
                <span className="hub-actions">
                  {canPreview && (
                    <button type="button" className="icon-btn icon-btn-sm" onClick={() => onOpenFile(it)} aria-label={t("read.openNamed", { name })} title={t("read.open")}>
                      <Eye size={14} strokeWidth={1.75} aria-hidden />
                    </button>
                  )}
                  <button type="button" className="icon-btn icon-btn-sm" onClick={() => onSave(it)} aria-label={t("read.saveNamed", { name })} title={t("read.save")}>
                    <Download size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                  <button type="button" className="icon-btn icon-btn-sm" onClick={() => onOpenMessage(it)} aria-label={t("hub.openMessage", { subject: it.subject || t("list.noSubject") })} title={t("hub.openMessageShort")}>
                    <Mail size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
