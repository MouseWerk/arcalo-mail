import { useT, type TKey } from "../lib/i18n";
import type { LanguageChoice, ThemeChoice } from "../lib/types";

const THEMES: [ThemeChoice, TKey][] = [
  ["system", "theme.system"],
  ["light", "theme.light"],
  ["dark", "theme.dark"],
  ["contrast", "theme.contrast"],
];

const LANGS: [LanguageChoice, TKey][] = [
  ["system", "theme.system"],
  ["de", "lang.de"],
  ["en", "lang.en"],
];

type Props = {
  theme: ThemeChoice;
  language: LanguageChoice;
  onTheme: (t: ThemeChoice) => void;
  onLanguage: (l: LanguageChoice) => void;
};

/** Appearance and language at the foot of the sidebar; a change applies at once. */
export function Preferences({ theme, language, onTheme, onLanguage }: Props) {
  const t = useT();
  return (
    <div className="prefs">
      <label className="pref">
        <span className="pref-label">{t("theme.label")}</span>
        <select className="select" value={theme} onChange={(e) => onTheme(e.target.value as ThemeChoice)}>
          {THEMES.map(([v, k]) => (
            <option key={v} value={v}>
              {t(k)}
            </option>
          ))}
        </select>
      </label>
      <label className="pref">
        <span className="pref-label">{t("lang.label")}</span>
        <select className="select" value={language} onChange={(e) => onLanguage(e.target.value as LanguageChoice)}>
          {LANGS.map(([v, k]) => (
            <option key={v} value={v}>
              {t(k)}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
