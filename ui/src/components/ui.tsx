// Small building blocks shared by the views: dialogs, menus, switches, segmented controls.

import { Check, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { useT } from "../lib/i18n";

/** A modal dialog: a scrim, focus inside, Esc and the close button close it. */
export function Dialog({
  title,
  description,
  onClose,
  children,
  footer,
  wide,
  className,
  above,
}: {
  title: string;
  description?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  className?: string;
  /** Over the compose sheet (a question it asked). */
  above?: boolean;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>("[autofocus], input, select, textarea, button:not(.dialog-close)");
    (first ?? ref.current)?.focus();
    return () => before?.focus?.();
  }, []);
  return (
    <div className={`overlay${above ? " overlay-above" : ""}`} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={ref}
        className={`dialog${wide ? " dialog-wide" : ""}${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
          if (e.key === "Tab") trapFocus(e, ref.current);
        }}
      >
        <header className="dialog-head">
          <div>
            <h2 className="dialog-title">{title}</h2>
            {description && <p className="dialog-desc">{description}</p>}
          </div>
          <button type="button" className="icon-btn dialog-close" onClick={onClose} aria-label={t("common.close")}>
            <X size={16} strokeWidth={1.75} aria-hidden />
          </button>
        </header>
        <div className="dialog-body">{children}</div>
        {footer && <footer className="dialog-foot">{footer}</footer>}
      </div>
    </div>
  );
}

/**
 * Fields that save when they are left (`onBlur`) save too when the view closes with them still
 * changed (Esc, Ctrl+, or a click beside the settings remove them without a blur in some
 * engines).
 */
export function useSaveOnLeave(changed: boolean, save: () => void) {
  const latest = useRef({ changed, save });
  latest.current = { changed, save };
  useEffect(
    () => () => {
      if (latest.current.changed) latest.current.save();
    },
    [],
  );
}

/**
 * Gives the focus back to where it was (the button that opened a menu, the list) when the popup
 * closes, unless it went somewhere on purpose meanwhile (a dialog the popup opened).
 */
export function useRestoreFocus(root: RefObject<HTMLElement | null>) {
  // Taken while rendering: the popup moves the focus into itself before effects run.
  const [before] = useState(() => (typeof document === "undefined" ? null : (document.activeElement as HTMLElement | null)));
  useEffect(() => {
    const el = root.current;
    return () => {
      const now = document.activeElement;
      if (before && before !== document.body && before.isConnected && (!now || now === document.body || el?.contains(now))) before.focus();
    };
  }, []);
}

/** Keeps Tab inside a dialog. */
export function trapFocus(e: { shiftKey: boolean; preventDefault: () => void }, root: HTMLElement | null) {
  if (!root) return;
  const items = Array.from(root.querySelectorAll<HTMLElement>("button, input, select, textarea, a[href], [tabindex]:not([tabindex='-1']), [contenteditable='true']")).filter(
    (el) => !el.hasAttribute("disabled") && el.offsetParent !== null,
  );
  if (!items.length) return;
  const first = items[0];
  const last = items[items.length - 1];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}

/** Asks for one line of text (a folder name). */
export function PromptDialog({
  title,
  label,
  initial = "",
  confirm,
  onDone,
  onClose,
}: {
  title: string;
  label: string;
  initial?: string;
  confirm: string;
  onDone: (value: string) => Promise<void> | void;
  onClose: () => void;
}) {
  const t = useT();
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    if (!value.trim()) return;
    try {
      await onDone(value.trim());
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <Dialog
      title={title}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" onClick={() => void submit()} disabled={!value.trim()}>
            {confirm}
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{label}</span>
        <input
          className="input"
          value={value}
          autoFocus
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void submit()}
        />
      </label>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

/** Asks before something that cannot be taken back. */
export function ConfirmDialog({
  title,
  text,
  detail,
  confirm,
  danger,
  onConfirm,
  onClose,
  above,
}: {
  title: string;
  text: string;
  /** Shown as it is below the text (an address the action goes to). */
  detail?: string;
  confirm: string;
  danger?: boolean;
  onConfirm: () => void;
  onClose: () => void;
  /** Over the compose sheet. */
  above?: boolean;
}) {
  const t = useT();
  return (
    <Dialog
      title={title}
      above={above}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className={`btn ${danger ? "btn-danger" : "btn-primary"}`}
            onClick={() => {
              onConfirm();
              onClose();
            }}
          >
            {confirm}
          </button>
        </>
      }
    >
      <p className="dialog-text">{text}</p>
      {detail && <p className="dialog-detail">{detail}</p>}
    </Dialog>
  );
}

export type MenuItem =
  | {
      label: string;
      icon?: ReactNode;
      onSelect: () => void;
      danger?: boolean;
      disabled?: boolean;
      /** A quiet note on the right (the time a preset stands for). */
      hint?: string;
      /** A choice that is in effect (a check mark, `menuitemradio`). */
      checked?: boolean;
    }
  | "separator"
  | { heading: string };

/** A popup menu at a point (a context menu) or below an element; arrows, Enter and Esc work. */
export function Menu({ items, at, onClose, label }: { items: MenuItem[]; at: { x: number; y: number }; onClose: () => void; label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(at);
  useRestoreFocus(ref);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ x: Math.min(at.x, window.innerWidth - r.width - 8), y: Math.min(at.y, window.innerHeight - r.height - 8) });
    el.querySelector<HTMLElement>("[role^='menuitem']:not([disabled])")?.focus();
  }, [at]);
  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  const menuItems = () => Array.from(ref.current?.querySelectorAll<HTMLElement>("[role^='menuitem']:not([disabled])") ?? []);
  const move = (dir: number) => {
    const list = menuItems();
    const i = list.indexOf(document.activeElement as HTMLElement);
    list[(i + dir + list.length) % list.length]?.focus();
  };
  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      style={{ left: pos.x, top: pos.y }}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        } else if (e.key === "Home" || e.key === "End") {
          e.preventDefault();
          const list = menuItems();
          (e.key === "Home" ? list[0] : list[list.length - 1])?.focus();
        } else if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        } else if (e.key === "Tab") {
          // Tab leaves the menu: it closes, the focus goes back to what opened it.
          e.preventDefault();
          onClose();
        }
      }}
    >
      {items.map((it, i) =>
        it === "separator" ? (
          <div key={i} className="menu-sep" role="separator" />
        ) : "heading" in it ? (
          <div key={i} className="menu-heading" role="presentation">
            {it.heading}
          </div>
        ) : (
          <button
            key={i}
            type="button"
            role={it.checked === undefined ? "menuitem" : "menuitemradio"}
            aria-checked={it.checked}
            className={`menu-item${it.danger ? " menu-danger" : ""}`}
            disabled={it.disabled}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            {it.icon}
            <span className="menu-label">{it.label}</span>
            {it.hint && <span className="menu-hint">{it.hint}</span>}
            {it.checked && <Check size={14} strokeWidth={2} aria-hidden className="menu-check" />}
          </button>
        ),
      )}
    </div>
  );
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} className={`switch${checked ? " on" : ""}`} onClick={() => onChange(!checked)}>
      <span className="switch-knob" />
    </button>
  );
}

export function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: [T, string][]; onChange: (v: T) => void; label: string }) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map(([v, text]) => (
        <button key={v} type="button" role="radio" aria-checked={v === value} className={v === value ? "on" : undefined} onClick={() => onChange(v)}>
          {text}
        </button>
      ))}
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return <span className="spinner" role={label ? "status" : undefined} aria-label={label} />;
}
