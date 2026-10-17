// The contact card: a person's picture, name and addresses (from an address book where there
// is one), the recent mail with them, the files they sent, and „Neue Mail“.

import { BookUser, Building2, Copy, Download, Mail, MailPlus, Phone } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { currentLang, useT } from "../lib/i18n";
import { fileSize, listDate, now } from "../lib/format";
import { showToast } from "../lib/toasts";
import type { CardFile, ContactCard as Card, MessageSummary } from "../lib/types";
import { Avatar } from "./Avatar";
import { kindIcon } from "./AttachmentHub";
import { Dialog, Spinner } from "./ui";

function fileKind(f: CardFile) {
  const m = f.mime_type;
  if (m.startsWith("image/")) return kindIcon("images");
  if (m === "application/pdf") return kindIcon("pdf");
  if (/sheet|excel|csv/.test(m)) return kindIcon("spreadsheets");
  if (/zip|compressed/.test(m)) return kindIcon("archives");
  return kindIcon("documents");
}

export function ContactCardDialog({
  email,
  name,
  onClose,
  onCompose,
  onOpen,
  onSave,
}: {
  email: string;
  name: string | null;
  onClose: () => void;
  onCompose: (to: string) => void;
  onOpen: (m: MessageSummary | { id: number }) => void;
  onSave: (f: CardFile) => void;
}) {
  const t = useT();
  const lang = currentLang();
  const [card, setCard] = useState<Card | null>(null);
  useEffect(() => {
    void api.contactCard(email).then(setCard);
  }, [email]);
  const shown = card?.name || name || email;
  const to = shown && shown !== email ? `${/[,;"]/.test(shown) ? `"${shown.replace(/"/g, "")}"` : shown} <${email}>` : email;
  return (
    <Dialog title={t("contact.title")} onClose={onClose} wide className="contact-card">
      <div className="contact-head">
        <Avatar who={{ name: shown, email }} size="lg" />
        <div className="contact-who">
          <div className="contact-name">{shown}</div>
          <div className="contact-email">
            <span>{email}</span>
            <button
              type="button"
              className="icon-btn icon-btn-xs"
              aria-label={t("contact.copy")}
              title={t("contact.copy")}
              onClick={() => void navigator.clipboard?.writeText(email).then(() => showToast({ title: t("contact.copied"), duration: 2000 }))}
            >
              <Copy size={12} strokeWidth={1.75} aria-hidden />
            </button>
          </div>
          {card?.org && (
            <div className="contact-line muted">
              <Building2 size={13} strokeWidth={1.75} aria-hidden />
              <span>{card.org}</span>
            </div>
          )}
          {card?.phones.map((p) => (
            <div key={p} className="contact-line muted">
              <Phone size={13} strokeWidth={1.75} aria-hidden />
              <span>{p}</span>
            </div>
          ))}
          {card?.emails.map((e) => (
            <div key={e} className="contact-line muted">
              <Mail size={13} strokeWidth={1.75} aria-hidden />
              <span>{e}</span>
            </div>
          ))}
          {card?.book && (
            <div className="contact-line muted">
              <BookUser size={13} strokeWidth={1.75} aria-hidden />
              <span>{t("contact.from", { book: card.book })}</span>
            </div>
          )}
        </div>
        <button
          type="button"
          className="btn btn-primary"
          onClick={() => {
            onClose();
            onCompose(to);
          }}
        >
          <MailPlus size={15} strokeWidth={1.75} aria-hidden />
          <span>{t("contact.newMail")}</span>
        </button>
      </div>
      {!card ? (
        <div className="contact-loading">
          <Spinner label={t("contact.loading")} />
        </div>
      ) : (
        <div className="contact-body">
          <p className="muted contact-counts">{t("contact.counts", { received: card.received, sent: card.sent })}</p>
          <section aria-label={t("contact.recent")}>
            <h3 className="contact-section">{t("contact.recent")}</h3>
            {card.recent.length === 0 ? (
              <p className="muted">{t("contact.noMail")}</p>
            ) : (
              <ul className="contact-list">
                {card.recent.map((m) => (
                  <li key={m.id}>
                    <button
                      type="button"
                      className="contact-row"
                      onClick={() => {
                        onClose();
                        onOpen(m);
                      }}
                    >
                      <span className={`contact-row-subject${m.seen ? "" : " unread"}`}>{m.subject || t("list.noSubject")}</span>
                      <span className="muted contact-row-date">{listDate(m.date, now(), lang)}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <section aria-label={t("contact.files")}>
            <h3 className="contact-section">{t("contact.files")}</h3>
            {card.files.length === 0 ? (
              <p className="muted">{t("contact.noFiles")}</p>
            ) : (
              <ul className="contact-list">
                {card.files.map((f) => {
                  const Icon = fileKind(f);
                  const fname = f.filename ?? t("read.unnamed");
                  return (
                    <li key={f.id} className="contact-file">
                      <button
                        type="button"
                        className="contact-row"
                        onClick={() => {
                          onClose();
                          onOpen({ id: f.message_id });
                        }}
                      >
                        <Icon size={15} strokeWidth={1.5} aria-hidden />
                        <span className="contact-row-subject">{fname}</span>
                        <span className="muted contact-row-date">
                          {fileSize(f.size, lang)} · {listDate(f.date, now(), lang)}
                        </span>
                      </button>
                      <button type="button" className="icon-btn icon-btn-sm" onClick={() => onSave(f)} aria-label={t("read.saveNamed", { name: fname })} title={t("read.save")}>
                        <Download size={14} strokeWidth={1.75} aria-hidden />
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </div>
      )}
    </Dialog>
  );
}
