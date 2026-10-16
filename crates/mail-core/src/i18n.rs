//! Backend texts in German and English, as in Arcalo: `tr!("Deutsch", "English")` for a fixed
//! text and `trf!("„{name}“ fehlt", "“{name}” is missing", name = …)` for one with values. The
//! translation check of the UI (`ui/src/lib/translations.test.ts`) reads both literals from the
//! source, so they must stay literals.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    De,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(1);

/// Sets the language of backend texts (the shell calls it at start and when the setting changes).
pub fn set_lang(lang: Lang) {
    LANG.store(if lang == Lang::De { 0 } else { 1 }, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 0 { Lang::De } else { Lang::En }
}

/// "de" for a German locale tag („de“, „de-AT“, „de_CH.UTF-8“), else "en".
pub fn lang_from_locale(tag: &str) -> Lang {
    let t = tag.trim().to_ascii_lowercase();
    let rest = t.strip_prefix("de").unwrap_or("x");
    if rest.is_empty() || rest.starts_with(['-', '_', '.', '@']) { Lang::De } else { Lang::En }
}

/// A fixed text in the current language.
#[macro_export]
macro_rules! tr {
    ($de:literal, $en:literal) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::De => $de,
            $crate::i18n::Lang::En => $en,
        }
    };
}

/// A text with values in the current language; arguments as for `format!`.
#[macro_export]
macro_rules! trf {
    ($de:literal, $en:literal $(, $($arg:tt)*)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::De => format!($de $(, $($arg)*)?),
            $crate::i18n::Lang::En => format!($en $(, $($arg)*)?),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags() {
        assert_eq!(lang_from_locale("de"), Lang::De);
        assert_eq!(lang_from_locale("de-AT"), Lang::De);
        assert_eq!(lang_from_locale("de_CH.UTF-8"), Lang::De);
        assert_eq!(lang_from_locale("en-US"), Lang::En);
        assert_eq!(lang_from_locale("dex"), Lang::En);
        assert_eq!(lang_from_locale(""), Lang::En);
    }
}
