// Small building blocks shared by the views: dialogs, menus, switches, segmented controls.

import { Check, X } from "lucide-react";
import { Children, Fragment, isValidElement, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useT } from "../lib/i18n";

/** A modal dialog: a scrim, focus inside, Esc and the close button close it. */
// Sheets, menus and dialogs over a screen: Android's Back closes the topmost of them first
// (App.tsx keeps a history entry while one is open).
const backLayers: { close: () => void }[] = [];
export const LAYERS_EVENT = "arcalo-layers";

/** How many layers are open. */
export const openLayers = () => backLayers.length;

/** Closes the topmost layer; false when none is open. */
export function closeTopLayer(): boolean {
  const top = backLayers[backLayers.length - 1];
  if (!top) return false;
  top.close();
  return true;
}

function useBackLayer(close: () => void) {
  const ref = useRef(close);
  ref.current = close;
  useEffect(() => {
    const layer = { close: () => ref.current() };
    backLayers.push(layer);
    window.dispatchEvent(new Event(LAYERS_EVENT));
    return () => {
      const i = backLayers.indexOf(layer);
      if (i >= 0) backLayers.splice(i, 1);
      window.dispatchEvent(new Event(LAYERS_EVENT));
    };
  }, []);
}

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
  useBackLayer(onClose);
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
      /** The value a choice of a `Select` stands for (data-value, for tests). */
      value?: string;
    }
  | "separator"
  | { heading: string };

/** A popup menu at a point (a context menu) or below an element; arrows, Enter and Esc work. */
export function Menu({ items, at, onClose, label }: { items: MenuItem[]; at: { x: number; y: number }; onClose: () => void; label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(at);
  // On a phone a menu is a sheet from the bottom edge, in reach of the thumb (app.css).
  const [sheet] = useState(() => typeof document !== "undefined" && document.documentElement.dataset.layout === "phone");
  useRestoreFocus(ref);
  useBackLayer(onClose);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (!sheet) {
      const r = el.getBoundingClientRect();
      setPos({ x: Math.min(at.x, window.innerWidth - r.width - 8), y: Math.min(at.y, window.innerHeight - r.height - 8) });
    }
    (el.querySelector<HTMLElement>("[aria-checked='true']:not([disabled])") ?? el.querySelector<HTMLElement>("[role^='menuitem']:not([disabled])"))?.focus({ preventScroll: true });
  }, [at, sheet]);
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
  // On the page itself, above everything: inside a dialog it would be clipped by the dialog's
  // scrolling body and drawn under its neighbours.
  return createPortal(
    <>
      {sheet && <div className="sheet-scrim" aria-hidden />}
      <div
        ref={ref}
        className={sheet ? "menu menu-sheet" : "menu"}
        role="menu"
        aria-label={label}
        style={sheet ? undefined : { left: pos.x, top: pos.y }}
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
              data-value={it.value}
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
    </>,
    document.body,
  );
}

/** What a `Select` hands to `onChange`: the shape of a `<select>`'s change event, so a field
 *  reads `e.target.value` either way. */
export type SelectChange = { target: { value: string } };

type OptionProps = { value?: string | number; disabled?: boolean; children?: ReactNode };

/** The `<option>` children of a `Select` (also inside arrays and fragments), as value and label. */
function optionsOf(children: ReactNode): { value: string; label: string; disabled: boolean }[] {
  const out: { value: string; label: string; disabled: boolean }[] = [];
  const text = (n: ReactNode): string =>
    n == null || typeof n === "boolean"
      ? ""
      : typeof n === "string" || typeof n === "number"
        ? String(n)
        : Array.isArray(n)
          ? n.map(text).join("")
          : isValidElement<{ children?: ReactNode }>(n)
            ? text(n.props.children)
            : "";
  const walk = (n: ReactNode) =>
    Children.forEach(n, (c) => {
      if (!isValidElement(c)) return;
      if (c.type === "option") {
        const p = c.props as OptionProps;
        const label = text(p.children);
        out.push({ value: p.value === undefined ? label : String(p.value), label, disabled: !!p.disabled });
      } else if (c.type === Fragment) walk((c.props as { children?: ReactNode }).children);
    });
  walk(children);
  return out;
}

/**
 * A choice from a list, drawn by the app instead of the system's `<select>` (whose stock
 * dropdown does not match the app on Android, Windows or Linux): a field-like button that opens
 * the choices as a menu, a sheet from the bottom edge on a phone. Takes `<option>` children and
 * the props of a `<select>`.
 */
export function Select({
  value,
  onChange,
  children,
  className,
  id,
  disabled,
  title,
  "aria-label": ariaLabel,
}: {
  value: string | number;
  onChange: (e: SelectChange) => void;
  children: ReactNode;
  className?: string;
  id?: string;
  disabled?: boolean;
  title?: string;
  "aria-label"?: string;
}) {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const ref = useRef<HTMLButtonElement>(null);
  const options = optionsOf(children);
  const current = options.find((o) => o.value === String(value));
  const open = () => {
    const r = ref.current?.getBoundingClientRect();
    if (r) setAt({ x: r.left, y: r.bottom + 4 });
  };
  // The field's name: its aria-label, else the text of its <label for>.
  const name = () => ariaLabel ?? (id && typeof document !== "undefined" ? document.querySelector(`label[for="${id}"]`)?.textContent ?? "" : "");
  const sheet = typeof document !== "undefined" && document.documentElement.dataset.layout === "phone";
  return (
    <>
      <button
        ref={ref}
        type="button"
        id={id}
        className={`${className ?? "select"} select-btn`}
        role="combobox"
        aria-haspopup="menu"
        aria-expanded={at !== null}
        aria-label={ariaLabel}
        title={title}
        disabled={disabled}
        data-value={String(value)}
        data-count={options.length}
        onClick={open}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            open();
          }
        }}
      >
        <span className="select-text">{current?.label ?? ""}</span>
      </button>
      {at && (
        <Menu
          at={at}
          label={name()}
          onClose={() => setAt(null)}
          items={[
            ...(sheet && name() ? [{ heading: name() }] : []),
            ...options.map((o) => ({ label: o.label, value: o.value, disabled: o.disabled, checked: o.value === String(value), onSelect: () => onChange({ target: { value: o.value } }) })),
          ]}
        />
      )}
    </>
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
  return (
    <svg className="spinner" viewBox="0 0 16 16" width="16" height="16" role={label ? "status" : undefined} aria-label={label} aria-hidden={label ? undefined : true} focusable="false">
      <circle className="spinner-track" cx="8" cy="8" r="7" />
      <path className="spinner-arc" d="M8 1a7 7 0 0 1 7 7" />
    </svg>
  );
}
