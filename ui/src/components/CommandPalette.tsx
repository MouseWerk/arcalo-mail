// Ctrl/Cmd+K: the commands of the app, the folders of every account and a search for what was
// typed, filtered as you type, run with Enter.

import { useEffect, useMemo, useRef, useState, type ComponentType } from "react";
import { useT } from "../lib/i18n";
import { score } from "../lib/palette";

type IconType = ComponentType<{ size?: number; strokeWidth?: number; "aria-hidden"?: boolean }>;

export type Command = {
  id: string;
  label: string;
  /** Words that find it too (the folder's account, the English name of a setting). */
  keywords?: string;
  hint?: string;
  icon: IconType;
  run: () => void;
};

export function CommandPalette({ commands, search, onClose }: { commands: Command[]; search: (q: string) => Command | null; onClose: () => void }) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  const shown = useMemo(() => {
    const q = query.trim();
    const hits = commands
      .map((c) => ({ c, s: score(`${c.label} ${c.keywords ?? ""}`, q) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => a.s - b.s)
      .map((x) => x.c);
    const s = q ? search(q) : null;
    return s ? [...hits, s] : hits;
  }, [commands, query, search]);

  useEffect(() => setIndex(0), [query]);
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [index]);

  const run = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    c.run();
  };

  return (
    <div className="overlay palette-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={t("palette.label")}>
        <input
          className="palette-input"
          autoFocus
          value={query}
          placeholder={t("palette.placeholder")}
          aria-label={t("palette.placeholder")}
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={shown[index] ? `palette-${shown[index].id}` : undefined}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              onClose();
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex((i) => Math.min(shown.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              run(shown[index]);
            }
          }}
        />
        {shown.length === 0 ? (
          <p className="palette-empty">{t("palette.empty")}</p>
        ) : (
          <ul id="palette-list" ref={listRef} className="palette-list" role="listbox" aria-label={t("palette.label")}>
            {shown.map((c, i) => {
              const Icon = c.icon;
              return (
                <li
                  key={c.id}
                  id={`palette-${c.id}`}
                  role="option"
                  aria-selected={i === index}
                  className="palette-item"
                  onMouseMove={() => setIndex(i)}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    run(c);
                  }}
                >
                  <Icon size={15} strokeWidth={1.75} aria-hidden />
                  <span className="palette-label">{c.label}</span>
                  {c.hint && <span className="palette-hint">{c.hint}</span>}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
