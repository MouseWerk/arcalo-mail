// The opening animation (markup in index.html, styles in public/splash.css), as Arcalo's: the mark
// draws itself while the app loads. It never holds the app back: when the app is ready (body.ready)
// it fades out at once, however far it got. Off when switched off (Settings → Darstellung,
// „Startanimation“) and under WebDriver; the finished mark without movement with reduced motion.

const KEY = "arcalo-mail.splash";
const FADE_MS = 180;
/** Never longer than this, whatever happens while loading. */
const LONGEST_MS = 8000;

export interface SplashPrefs {
  /** Switched off in the settings. */
  off?: boolean;
  /** The colors of the theme of the last start. */
  bg?: string;
  ink?: string;
  seal?: string;
}

const HEX = /^#[0-9a-f]{6}$/i;

function read(): SplashPrefs {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as SplashPrefs;
  } catch {
    return {};
  }
}

let last = "";
/** Remembered for the next start; merged with what is stored, written only on a change. */
export function rememberSplash(p: SplashPrefs) {
  const next = JSON.stringify({ ...read(), ...p });
  if (next === last) return;
  last = next;
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // Private mode or full storage: the next start uses the system's colors.
  }
}

/** A color of the page as #rrggbb, for the next start's splash. */
export function cssHex(name: string): string | undefined {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return HEX.test(v) ? v : undefined;
}

/** Shows the splash until the app is ready (or removes it at once when it is off). */
export function startSplash() {
  const el = document.getElementById("splash");
  if (!el) return;
  const p = read();
  const forced = localStorage.getItem("arcalo-mail.splash-test") === "1";
  if (p.off || (navigator.webdriver && !forced)) {
    el.remove();
    return;
  }
  // Only well-formed colors: the stored value ends up in a style property.
  for (const [key, v] of [["bg", p.bg], ["ink", p.ink], ["seal", p.seal]] as const) {
    if (v && HEX.test(v)) el.style.setProperty(`--splash-${key}`, v);
  }
  let done = false;
  const hide = () => {
    if (done) return;
    done = true;
    watch.disconnect();
    el.classList.add("out");
    setTimeout(() => el.remove(), FADE_MS);
  };
  const watch = new MutationObserver(() => document.body.classList.contains("ready") && hide());
  watch.observe(document.body, { attributes: true, attributeFilter: ["class"] });
  if (document.body.classList.contains("ready")) hide();
  setTimeout(hide, LONGEST_MS);
}
