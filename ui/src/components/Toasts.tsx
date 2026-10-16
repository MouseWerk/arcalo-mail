import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useT } from "../lib/i18n";
import { closeToast, useToasts } from "../lib/toasts";

/** The toasts at the bottom: a tinted card with an icon, never a colored bar. */
export function Toasts() {
  const t = useT();
  const list = useToasts();
  return (
    <div className="toasts" aria-live="polite">
      {list.map((x) => {
        const Icon = x.tone === "danger" ? CircleAlert : x.tone === "success" ? CircleCheck : Info;
        return (
          <div key={x.id} className={`toast toast-${x.tone ?? "info"}`} role={x.tone === "danger" ? "alert" : "status"}>
            <Icon size={16} strokeWidth={1.75} aria-hidden className="toast-icon" />
            <div className="toast-body">
              <div className="toast-title">{x.title}</div>
              {x.detail && <div className="toast-detail">{x.detail}</div>}
            </div>
            {x.action && (
              <button
                type="button"
                className="btn btn-sm toast-action"
                onClick={() => {
                  closeToast(x.id);
                  x.action?.run();
                }}
              >
                {x.action.label}
              </button>
            )}
            <button type="button" className="icon-btn icon-btn-sm" onClick={() => closeToast(x.id)} aria-label={t("common.close")}>
              <X size={14} strokeWidth={1.75} aria-hidden />
            </button>
          </div>
        );
      })}
    </div>
  );
}
