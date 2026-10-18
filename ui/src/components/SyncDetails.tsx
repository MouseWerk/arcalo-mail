import { Copy } from "lucide-react";
import { useState } from "react";
import { api } from "../lib/api";
import { useT } from "../lib/i18n";
import { showToast } from "../lib/toasts";

/** „Sync-Details“ of an account: the steps of its last sync with their times, to copy into a report. */
export function SyncDetails({ accountId }: { accountId: number }) {
  const t = useT();
  const [text, setText] = useState<string | null>(null);
  const show = () => void api.syncDetails(accountId).then(setText);
  const copy = () => void navigator.clipboard?.writeText(text ?? "").then(() => showToast({ title: t("settings.syncDetailsCopied"), duration: 2000 }));
  return (
    <div className="set-card">
      <div className="set-row">
        <div className="set-text">
          <div className="set-label">{t("settings.syncDetails")}</div>
          <div className="set-desc">{t("settings.syncDetailsHint")}</div>
        </div>
        <div className="set-actions">
          <button type="button" className="btn" onClick={show}>
            {t(text === null ? "settings.syncDetailsShow" : "settings.syncDetailsRefresh")}
          </button>
          {text && (
            <button type="button" className="btn" onClick={copy}>
              <Copy size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("settings.syncDetailsCopy")}</span>
            </button>
          )}
        </div>
      </div>
      {text !== null && (
        <pre className="sync-details" aria-label={t("settings.syncDetails")}>
          {text || t("settings.syncDetailsEmpty")}
        </pre>
      )}
    </div>
  );
}
