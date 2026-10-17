// The phishing warning above a message (mail_core::phishing): a calm notice with the reasons,
// never a block. „Das ist in Ordnung“ silences it for the sender.

import { useState } from "react";
import { ShieldAlert } from "lucide-react";
import { api } from "../lib/api";
import { useT, type TKey, type TVars } from "../lib/i18n";
import { showToast } from "../lib/toasts";
import type { PhishingReason, PhishingWarning } from "../lib/types";

const CLAIMS: Record<string, TKey> = {
  bank: "phish.claim.bank",
  payment: "phish.claim.payment",
  parcel: "phish.claim.parcel",
  company: "phish.claim.company",
};

/** One reason as a sentence: its catalog key and the values for it. */
export function reasonText(r: PhishingReason): [TKey, TVars] {
  switch (r.reason) {
    case "lookalike":
      return ["phish.lookalike", { domain: r.domain, like: r.like }];
    case "punycode":
      return ["phish.punycode", { shown: r.shown, domain: r.domain }];
    case "reply_to":
      return ["phish.replyTo", { reply: r.reply_to, domain: r.from }];
    case "name_address":
      return ["phish.nameAddress", { shown: r.shown, actual: r.actual }];
    case "first_contact":
      return ["phish.firstContact", { claim: r.claim }];
    case "link":
      return r.more > 0 ? ["phish.linkMore", { shown: r.shown, target: r.target, n: r.more }] : ["phish.link", { shown: r.shown, target: r.target }];
    case "auth_failed":
      return [r.checks.length > 1 ? "phish.authBoth" : r.checks[0] === "spf" ? "phish.authSpf" : "phish.authDmarc", {}];
  }
}

export function PhishingNotice({ warning, onChanged }: { warning: PhishingWarning; onChanged: () => void }) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const claim = warning.reasons.find((r) => r.reason === "first_contact");
  const fine = async () => {
    setBusy(true);
    try {
      await api.phishingOk(warning.sender, true);
      showToast({
        title: t("phish.okDone", { email: warning.sender }),
        duration: 6000,
        action: {
          label: t("common.undo"),
          run: () => void api.phishingOk(warning.sender, false).then(onChanged),
        },
      });
      onChanged();
    } catch (e) {
      showToast({ title: String(e), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="notice notice-warning phish" role="note" aria-label={t("phish.title")} data-phishing>
      <ShieldAlert size={16} strokeWidth={1.75} aria-hidden />
      <div className="phish-body">
        <strong>{t("phish.title")}</strong>
        <ul className="phish-reasons">
          {warning.reasons.map((r, i) => {
            const [key, vars] = reasonText(r);
            return (
              <li key={i} data-reason={r.reason}>
                {t(key, vars)}
              </li>
            );
          })}
        </ul>
        <span className="phish-advice">{claim && claim.reason === "first_contact" ? t(CLAIMS[claim.kind] ?? "phish.claim.company") : t("phish.advice")}</span>
      </div>
      <span className="notice-actions">
        <button type="button" className="btn btn-sm btn-quiet" disabled={busy} onClick={() => void fine()} data-phishing-ok>
          {t("phish.ok")}
        </button>
      </span>
    </div>
  );
}
