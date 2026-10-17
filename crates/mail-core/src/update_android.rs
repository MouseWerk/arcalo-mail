//! Updates of the Android app installed from the GitHub release (the APK): the feed
//! `android-latest.json`, published next to the desktop's `latest.json` in every release, and the
//! decision whether a newer version is offered. As in Arcalo (`arcalo_core::update_android`), with
//! one difference: Arcalo Mail only tells about the new version and opens its download in the
//! browser; Android's own installer checks the APK and asks before it replaces the app. So the app
//! needs no permission to install packages.
//!
//! A copy installed from Google Play never asks the feed: Play updates it, and Play's policy does
//! not allow an app from Play to update itself any other way. The installer package Android
//! names for the app ([`from_store`]) decides it.

use serde::{Deserialize, Serialize};

use crate::tr;
use crate::update_feed::FeedError;

/// The Android feed of the newest release on GitHub.
pub const FEED_URL: &str = "https://github.com/MouseWerk/arcalo-mail/releases/latest/download/android-latest.json";

/// The application id of the app.
pub const PACKAGE: &str = "de.mousewerk.arcalo.mail";

/// At most this often the app asks the feed by itself (it also asks at every start).
pub const CHECK_EVERY_HOURS: i64 = 24;

/// Installer packages of app stores: a copy they installed is updated by them.
const STORES: [&str; 4] =
    ["com.android.vending", "com.google.android.feedback", "com.amazon.venezia", "com.sec.android.app.samsungapps"];

/// Whether the app came from a store (Google Play and others) that updates it itself. `installer`
/// is the installing package Android names (`None`: installed by hand, `adb`, a browser or a file
/// manager, which is the APK of the GitHub release).
pub fn from_store(installer: Option<&str>) -> bool {
    installer.map(str::trim).is_some_and(|i| STORES.contains(&i))
}

/// [`from_store`] for what Android said (`Err`: the question failed). Without an answer the
/// app counts as a store's: the update notice stays off rather than showing in a Play install.
pub fn from_store_or_unknown<E>(installer: std::result::Result<Option<&str>, E>) -> bool {
    installer.map_or(true, from_store)
}

/// `android-latest.json`: the release's version, its APK and how that APK is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidRelease {
    pub version: String,
    /// Android's version code of the APK (1.0.0 → 1000000, see [`version_code`]).
    pub version_code: u64,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    /// The application id inside the APK.
    pub package: String,
    pub url: String,
    /// Base64 of the APK's minisign `.sig` file (the desktop's update key).
    #[serde(default)]
    pub signature: Option<String>,
    /// Size of the APK in bytes.
    #[serde(default)]
    pub size: Option<u64>,
    /// SHA-256 of the APK's signing certificate (hex).
    #[serde(default)]
    pub signer: Option<String>,
}

/// Reads `android-latest.json`; a feed without a usable version, version code, package or HTTPS
/// address is invalid.
pub fn parse(text: &str) -> Result<AndroidRelease, FeedError> {
    let mut r: AndroidRelease =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| FeedError::Invalid(e.to_string()))?;
    r.version = r.version.trim().trim_start_matches('v').to_string();
    if semver::Version::parse(&r.version).is_err() {
        return Err(FeedError::Invalid(format!("version {}", r.version)));
    }
    if r.version_code == 0 {
        return Err(FeedError::Invalid("version_code 0".into()));
    }
    if r.package.trim().is_empty() {
        return Err(FeedError::Invalid("no package".into()));
    }
    if !r.url.to_ascii_lowercase().starts_with("https://") {
        return Err(FeedError::Invalid(format!("url {}", r.url)));
    }
    r.signer = r.signer.as_deref().and_then(normalize_cert);
    Ok(r)
}

/// Android's version code of `version` as the Tauri CLI computes it for the APK:
/// major × 1 000 000 + minor × 1 000 + patch (1.0.0 → 1000000).
pub fn version_code(version: &str) -> Option<u64> {
    let v = semver::Version::parse(version.trim().trim_start_matches('v')).ok()?;
    if v.minor >= 1000 || v.patch >= 1000 {
        return None;
    }
    Some(v.major * 1_000_000 + v.minor * 1_000 + v.patch)
}

/// A certificate digest as uppercase hex without separators (`ab:cd` and `ABCD` are the same);
/// `None` when it is not hex.
pub fn normalize_cert(raw: &str) -> Option<String> {
    let hex: String = raw.chars().filter(|c| !matches!(c, ':' | ' ' | '\t')).collect::<String>().to_ascii_uppercase();
    (!hex.is_empty() && hex.len().is_multiple_of(2) && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

/// The installed app as Android's package manager reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub package: String,
    pub version_code: u64,
    #[serde(default)]
    pub version_name: String,
    /// SHA-256 of the signing certificate (hex), when Android names it.
    #[serde(default)]
    pub signer: Option<String>,
    /// The package that installed the app (`com.android.vending` for Google Play).
    #[serde(default)]
    pub installer: Option<String>,
}

/// What a release means for the installed app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Offer {
    /// Nothing newer.
    UpToDate,
    /// A newer version that installs over this app.
    Update,
    /// A newer version signed with another APK key: Android refuses to install it over this one
    /// (the Play build, signed by Google, or a debug-signed test build).
    OtherSigner,
    /// The feed names another app (never offered).
    OtherPackage,
}

/// Whether `release` is an update for `installed`: the same package, a newer version and a higher
/// version code (Android never installs a lower or equal one over the app), and, when both are
/// known, the same signing certificate.
pub fn offer(installed: &Installed, release: &AndroidRelease) -> Offer {
    if release.package != installed.package {
        return Offer::OtherPackage;
    }
    let newer = crate::update::is_newer(&installed.version_name, &release.version)
        || (installed.version_name.is_empty() && release.version_code > installed.version_code);
    if !newer || release.version_code <= installed.version_code {
        return Offer::UpToDate;
    }
    match (installed.signer.as_deref().and_then(normalize_cert), release.signer.as_deref()) {
        (Some(mine), Some(theirs)) if mine != theirs => Offer::OtherSigner,
        _ => Offer::Update,
    }
}

/// The text for an update that is signed with another APK key than the installed app.
pub fn other_signer() -> &'static str {
    tr!(
        "Diese Version ist mit einem anderen App-Schlüssel signiert als die installierte App. Android installiert sie nur nach einer Deinstallation; deine Konten richtest du danach neu ein.",
        "This version is signed with another app key than the installed app. Android installs it only after an uninstall; you then set up your accounts again."
    )
}

/// Whether the daily check is due (`last` is the last check's time; none yet: due).
pub fn check_due(last: Option<chrono::DateTime<chrono::Utc>>, now: chrono::DateTime<chrono::Utc>) -> bool {
    match last {
        None => true,
        // A clock set back (a check "in the future") counts as due, so it never stops checking.
        Some(at) => now < at || now - at >= chrono::Duration::hours(CHECK_EVERY_HOURS),
    }
}

/// „Neu in Arcalo Mail“ after an update of the app: `seen` is the version the news was last shown
/// for (none on the first start), `current` the version running now. Returns the version the
/// update came from while its news waits to be shown, and the version to record now (`None`:
/// nothing to store). A first start shows nothing and records the version, so a new install does
/// not greet with news.
pub fn whats_new_from(seen: Option<&str>, current: &str) -> (Option<String>, Option<String>) {
    let current = current.trim();
    match seen.map(str::trim).filter(|s| !s.is_empty()) {
        None => (None, Some(current.to_owned())),
        Some(s) if crate::update::is_newer(s, current) => (Some(s.to_owned()), None),
        Some(s) if s == current => (None, None),
        // An older version installed again: recorded, nothing to show.
        Some(_) => (None, Some(current.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn release(version: &str, code: u64) -> AndroidRelease {
        AndroidRelease {
            version: version.into(),
            version_code: code,
            notes: None,
            pub_date: None,
            package: PACKAGE.into(),
            url: format!(
                "https://github.com/MouseWerk/arcalo-mail/releases/download/v{version}/ArcaloMail_{version}_android.apk"
            ),
            signature: None,
            size: Some(40_000_000),
            signer: Some("AB".repeat(32)),
        }
    }

    fn installed(version: &str, signer: Option<&str>) -> Installed {
        Installed {
            package: PACKAGE.into(),
            version_code: version_code(version).unwrap(),
            version_name: version.into(),
            signer: signer.map(str::to_string),
            installer: None,
        }
    }

    #[test]
    fn the_feed_is_read_strictly() {
        let ok = r#"{"version":"v1.0.1","version_code":1000001,"package":"de.mousewerk.arcalo.mail",
            "url":"https://github.com/x.apk","signer":"ab:cd"}"#;
        let r = parse(ok).unwrap();
        assert_eq!(r.version, "1.0.1");
        assert_eq!(r.signer.as_deref(), Some("ABCD"));
        for bad in [
            r#"{"version":"soon","version_code":1,"package":"p","url":"https://x"}"#,
            r#"{"version":"1.0.1","version_code":0,"package":"p","url":"https://x"}"#,
            r#"{"version":"1.0.1","version_code":1,"package":" ","url":"https://x"}"#,
            r#"{"version":"1.0.1","version_code":1,"package":"p","url":"http://x"}"#,
        ] {
            assert!(matches!(parse(bad), Err(FeedError::Invalid(_))), "{bad}");
        }
    }

    #[test]
    fn version_codes_as_the_tauri_cli_makes_them() {
        assert_eq!(version_code("1.0.0"), Some(1_000_000));
        assert_eq!(version_code("v1.2.3"), Some(1_002_003));
        assert_eq!(version_code("1.1000.0"), None);
    }

    #[test]
    fn only_a_newer_apk_with_the_same_key_is_offered() {
        let key = "AB".repeat(32);
        assert_eq!(offer(&installed("1.0.0", Some(&key)), &release("1.0.1", 1_000_001)), Offer::Update);
        assert_eq!(offer(&installed("1.0.1", Some(&key)), &release("1.0.1", 1_000_001)), Offer::UpToDate);
        assert_eq!(offer(&installed("1.0.0", Some(&key)), &release("1.0.1", 1_000_000)), Offer::UpToDate);
        assert_eq!(offer(&installed("1.0.0", Some("CD")), &release("1.0.1", 1_000_001)), Offer::OtherSigner);
        assert_eq!(
            offer(&installed("1.0.0", None), &release("1.0.1", 1_000_001)),
            Offer::Update,
            "unknown: Android decides"
        );
        let mut other = release("1.0.1", 1_000_001);
        other.package = "de.mousewerk.arcalo".into();
        assert_eq!(offer(&installed("1.0.0", Some(&key)), &other), Offer::OtherPackage);
    }

    #[test]
    fn a_store_install_never_asks_the_feed() {
        assert!(from_store(Some("com.android.vending")));
        assert!(from_store(Some(" com.android.vending ")));
        assert!(!from_store(None), "installed by hand: the APK of the release");
        assert!(!from_store(Some("com.google.android.packageinstaller")));
        assert!(!from_store(Some("com.android.chrome")));
        // Android could not say: the notice stays off (a Play install must never show it).
        assert!(from_store_or_unknown::<String>(Err("no answer".into())));
        assert!(!from_store_or_unknown::<String>(Ok(None)));
        assert!(from_store_or_unknown::<String>(Ok(Some("com.android.vending"))));
    }

    #[test]
    fn the_daily_check() {
        let now = Utc::now();
        assert!(check_due(None, now));
        assert!(!check_due(Some(now - Duration::hours(2)), now));
        assert!(check_due(Some(now - Duration::hours(25)), now));
        assert!(check_due(Some(now + Duration::hours(1)), now), "a clock set back");
    }

    #[test]
    fn news_after_an_update_once() {
        assert_eq!(whats_new_from(None, "1.0.0"), (None, Some("1.0.0".into())), "a new install");
        assert_eq!(whats_new_from(Some("1.0.0"), "1.0.1"), (Some("1.0.0".into()), None));
        assert_eq!(whats_new_from(Some("1.0.1"), "1.0.1"), (None, None));
        assert_eq!(whats_new_from(Some("1.0.2"), "1.0.1"), (None, Some("1.0.1".into())));
    }
}
