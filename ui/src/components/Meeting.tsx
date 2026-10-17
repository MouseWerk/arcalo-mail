// What a message says beyond its text: a meeting invitation as a card (time, place, attendees,
// what else is at that time, „Zusagen / Mit Vorbehalt / Absagen“), the „Beitreten“ button of an
// online meeting, and the lock and check of a signed or encrypted message with the files inside it.

import { useState } from "react";
import { CalendarCheck, CalendarDays, CalendarX, Check, CircleHelp, Download, FileLock2, Lock, MapPin, ShieldAlert, ShieldCheck, Users, Video, X } from "lucide-react";
import { api, openExternal, pickSavePath } from "../lib/api";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { fileSize } from "../lib/format";
import { showToast } from "../lib/toasts";
import { attendeeName, dateLocale, hhmm, isoDay } from "../lib/calendar";
import type { InvitationAnswer, InvitationCard as Card, MessageView, Protection } from "../lib/types";

const STATUS: Record<string, TKey> = { ACCEPTED: "invite.st.accepted", TENTATIVE: "invite.st.tentative", DECLINED: "invite.st.declined", "NEEDS-ACTION": "invite.st.open", DELEGATED: "invite.st.delegated" };

/** „Di., 14. Oktober · 10:00–11:00“ of an invitation. */
function whenText(start: Date, end: Date, allDay: boolean, t: ReturnType<typeof useT>) {
  const l = dateLocale();
  const day = start.toLocaleDateString(l, { weekday: "short", day: "numeric", month: "long", year: start.getFullYear() !== new Date().getFullYear() ? "numeric" : undefined });
  if (allDay) {
    const last = new Date(end.getTime() - 1);
    return isoDay(last) === isoDay(start) ? `${day} · ${t("cal.allDay")}` : `${day} – ${last.toLocaleDateString(l, { day: "numeric", month: "long" })}`;
  }
  return isoDay(start) === isoDay(end) ? `${day} · ${hhmm(start)}–${hhmm(end)}` : `${day} ${hhmm(start)} – ${end.toLocaleDateString(l, { day: "numeric", month: "short" })} ${hhmm(end)}`;
}

export function JoinButton({ url, kind, primary }: { url: string; kind: string; primary?: boolean }) {
  const t = useT();
  return (
    <button type="button" className={`btn btn-sm${primary ? " btn-primary" : ""} join-btn`} onClick={() => void openExternal(url).catch((e) => showToast({ title: t("calv.joinFailed"), detail: String(e), tone: "danger" }))} title={url}>
      <Video size={14} strokeWidth={1.75} aria-hidden />
      <span>{t("meeting.join", { kind })}</span>
    </button>
  );
}

/** The invitation in a message, with the answer and what else is at that time. */
export function InvitationCard({ view, onAnswered, onOpenCalendar }: { view: MessageView; onAnswered: () => void; onOpenCalendar: (key: string | null, start: string) => void }) {
  const t = useT();
  const card = view.invitation as Card;
  const inv = card.invitation;
  const [busy, setBusy] = useState<InvitationAnswer | null>(null);
  const [allPeople, setAllPeople] = useState(false);
  const start = new Date(inv.start);
  const end = new Date(inv.end);
  const cancelled = inv.method === "CANCEL";
  const reply = inv.method === "REPLY";
  const canAnswer = inv.method === "REQUEST" && !inv.mine && !card.outdated && !!inv.me;
  const answer = card.answer ?? (inv.my_status ? ({ ACCEPTED: "accepted", TENTATIVE: "tentative", DECLINED: "declined" } as Record<string, InvitationAnswer>)[inv.my_status] : undefined) ?? null;
  const people = allPeople ? inv.attendees : inv.attendees.slice(0, 6);
  const send = async (a: InvitationAnswer) => {
    setBusy(a);
    try {
      const r = await api.invitationAnswer(view.summary.id, a, null);
      const key: TKey = r.sent === "none" ? "invite.savedOnly" : "invite.sent";
      showToast({ title: t(a === "accepted" ? "invite.acceptedToast" : a === "tentative" ? "invite.tentativeToast" : "invite.declinedToast"), detail: t(key), tone: "success", duration: 3500 });
      onAnswered();
    } catch (e) {
      showToast({ title: t("invite.failed"), detail: String(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  const heading: TKey = cancelled ? "invite.cancelled" : reply ? "invite.reply" : inv.sequence > 0 && inv.method === "REQUEST" ? "invite.updated" : "invite.title";
  return (
    <section className={`invite-card${cancelled ? " cancelled" : ""}`} aria-label={t(heading)}>
      <div className="invite-head">
        {cancelled ? <CalendarX size={16} strokeWidth={1.75} aria-hidden /> : <CalendarDays size={16} strokeWidth={1.75} aria-hidden />}
        <span className="invite-kind">{t(heading)}</span>
        {card.outdated && <span className="invite-outdated">{t("invite.outdated")}</span>}
      </div>
      <h3 className="invite-title">{inv.title || t("list.noSubject")}</h3>
      <div className="invite-rows">
        <div className="invite-row">
          <CalendarDays size={14} strokeWidth={1.75} aria-hidden />
          <span className="invite-when">
            {whenText(start, end, inv.all_day, t)}
            {inv.recurring && <span className="muted"> · {t("calv.partOfSeries")}</span>}
          </span>
        </div>
        {inv.location && (
          <div className="invite-row">
            <MapPin size={14} strokeWidth={1.75} aria-hidden />
            <span>{inv.location}</span>
          </div>
        )}
        {inv.attendees.length > 0 && (
          <div className="invite-row">
            <Users size={14} strokeWidth={1.75} aria-hidden />
            <span className="invite-people">
              <span>
                {attendeeName(inv.organizer) || inv.organizer} <span className="muted">({t("calv.organizer")})</span>
              </span>
              {people.map((a) => (
                <span key={a.email} className="invite-person">
                  {a.name || a.email}
                  {a.status !== "NEEDS-ACTION" && STATUS[a.status] && <span className="muted"> · {t(STATUS[a.status])}</span>}
                </span>
              ))}
              {inv.attendees.length > people.length && (
                <button type="button" className="link-btn" onClick={() => setAllPeople(true)}>
                  {t("calv.more", { n: inv.attendees.length - people.length })}
                </button>
              )}
            </span>
          </div>
        )}
        {!cancelled && card.conflicts.length > 0 && (
          <div className="invite-row invite-conflicts" role="note">
            <CircleHelp size={14} strokeWidth={1.75} aria-hidden />
            <span>
              {t("invite.conflicts", { n: card.conflicts.length })}{" "}
              {card.conflicts.slice(0, 3).map((c, i) => (
                <span key={c.key} className="invite-conflict">
                  {i > 0 ? ", " : ""}
                  {c.title} <span className="muted">({hhmm(new Date(c.start))}–{hhmm(new Date(c.end))})</span>
                </span>
              ))}
            </span>
          </div>
        )}
        {!cancelled && card.conflicts.length === 0 && inv.method === "REQUEST" && !inv.all_day && (
          <div className="invite-row muted">
            <Check size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("invite.free")}</span>
          </div>
        )}
      </div>
      <div className="invite-actions">
        {canAnswer && (
          <span className="segmented invite-answer" role="group" aria-label={t("invite.answer")}>
            {(["accepted", "tentative", "declined"] as InvitationAnswer[]).map((a) => (
              <button key={a} type="button" aria-pressed={answer === a} className={answer === a ? "on" : undefined} disabled={!!busy} onClick={() => void send(a)}>
                {a === "accepted" ? <Check size={13} strokeWidth={2} aria-hidden /> : a === "declined" ? <X size={13} strokeWidth={2} aria-hidden /> : <CircleHelp size={13} strokeWidth={1.75} aria-hidden />}
                <span>{t(a === "accepted" ? "invite.accept" : a === "tentative" ? "invite.tentative" : "invite.decline")}</span>
              </button>
            ))}
          </span>
        )}
        {!canAnswer && answer && !cancelled && (
          <span className="invite-state">
            <CalendarCheck size={14} strokeWidth={1.75} aria-hidden />
            <span>{t(answer === "accepted" ? "invite.youAccepted" : answer === "tentative" ? "invite.youTentative" : "invite.youDeclined")}</span>
          </span>
        )}
        {inv.link && !cancelled && <JoinButton url={inv.link} kind={inv.link_kind ?? "Online"} primary={answer === "accepted"} />}
        {!cancelled && (
          <button type="button" className="btn btn-sm btn-quiet" onClick={() => onOpenCalendar(card.event_key, inv.start)}>
            <CalendarDays size={14} strokeWidth={1.75} aria-hidden />
            <span>{card.event_key ? t("invite.showInCalendar") : t("invite.openCalendar")}</span>
          </button>
        )}
      </div>
    </section>
  );
}

const SIGNATURE: Record<NonNullable<Protection["signature"]>, TKey> = {
  valid: "protect.sigValid",
  untrusted: "protect.sigUntrusted",
  mismatch: "protect.sigMismatch",
  unknown_key: "protect.sigUnknown",
  invalid: "protect.sigInvalid",
};

/** The lock and the check of a protected message, quiet when all is well. */
export function ProtectionBadges({ p }: { p: Protection }) {
  const t = useT();
  const method = p.method === "pgp" ? "OpenPGP" : "S/MIME";
  const good = p.signature === "valid";
  const bad = p.signature === "invalid" || p.signature === "mismatch";
  return (
    <div className="protect-badges" role="status">
      {p.encrypted && (
        <span className={`protect-badge${p.decrypted ? "" : " warn"}`} title={p.error ?? undefined}>
          <Lock size={13} strokeWidth={1.75} aria-hidden />
          <span>{p.decrypted ? t("protect.encrypted", { method }) : t("protect.notDecrypted", { method })}</span>
        </span>
      )}
      {p.signed && p.signature && (
        <span className={`protect-badge${good ? " ok" : bad ? " warn" : ""}`} title={p.fingerprint ?? undefined}>
          {good ? <ShieldCheck size={13} strokeWidth={1.75} aria-hidden /> : <ShieldAlert size={13} strokeWidth={1.75} aria-hidden />}
          <span>{t(SIGNATURE[p.signature], { signer: p.signer ?? "–", method })}</span>
        </span>
      )}
      {p.encrypted && p.decrypted && <span className="muted protect-note">{t("protect.noRemote")}</span>}
      {p.error && !p.decrypted && <span className="muted protect-note">{p.error}</span>}
    </div>
  );
}

/** The files inside an encrypted message: saved through the backend, decrypted again. */
export function ProtectedFiles({ view }: { view: MessageView }) {
  const t = useT();
  const lang = currentLang();
  if (!view.protected_files.length) return null;
  const save = async (index: number, name: string) => {
    const path = await pickSavePath(name);
    if (!path) return;
    try {
      await api.saveProtectedAttachment(view.summary.id, index, path);
      showToast({ title: t("toast.saved", { name }), duration: 2500 });
    } catch (e) {
      showToast({ title: t("read.saveFailed"), detail: String(e), tone: "danger" });
    }
  };
  return (
    <div className="attachments" role="group" aria-label={t("read.attachments", { n: view.protected_files.length })}>
      <div className="attachments-head">
        <FileLock2 size={14} strokeWidth={1.75} aria-hidden />
        <span>{t("protect.files", { n: view.protected_files.length })}</span>
      </div>
      <ul className="attachment-list">
        {view.protected_files.map((f) => {
          const name = f.filename ?? t("read.unnamed");
          return (
            <li key={f.index} className="attachment">
              <span className="attachment-main">
                <span className="attachment-icon">
                  <FileLock2 size={18} strokeWidth={1.5} aria-hidden />
                </span>
                <span className="attachment-text">
                  <span className="attachment-name">{name}</span>
                  <span className="attachment-size">{fileSize(f.size, lang)}</span>
                </span>
              </span>
              <button type="button" className="icon-btn icon-btn-sm" onClick={() => void save(f.index, name)} aria-label={t("read.saveNamed", { name })} title={t("read.save")}>
                <Download size={14} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
