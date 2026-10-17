//! Updates of the desktop app, the decisions that do not need the shell: which version is newer,
//! why a copy cannot replace itself, what the updater remembers between starts (a skipped
//! version, „Später erinnern“, the „Neu in Arcalo Mail“ shown), and the note of the first start
//! after an update. As in Arcalo (`arcalo_core::update`, `update_state`).
//!
//! The shell switches the updater on only when the release build compiled in the public key of
//! the update signing key pair (`ARCALO_MAIL_UPDATER_PUBKEY`, from `plugins.updater.pubkey` in
//! `src-tauri/tauri.conf.json`). Development and test builds have no key: they never contact the
//! update feed. The feed and the downloads are `update_feed`; the phones' feed `update_android`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::tr;

/// The GitHub repository the releases are published to.
pub const REPOSITORY: &str = "MouseWerk/arcalo-mail";

/// The update feed of the desktop: `latest.json` of the newest release. Kept equal to
/// `plugins.updater.endpoints` in `tauri.conf.json` (a test in the shell checks it).
pub const FEED_URL: &str = "https://github.com/MouseWerk/arcalo-mail/releases/latest/download/latest.json";

/// What `plugins.updater.pubkey` says until the owner put the real public key there
/// (docs/release.md). A build with it has no updater; the release workflow refuses to build.
pub const PUBKEY_PLACEHOLDER: &str = "REPLACE_WITH_THE_UPDATE_PUBLIC_KEY";

/// The compiled-in public key, if it is usable: blank values and the placeholder count as "not
/// configured".
pub fn configured_pubkey(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|k| !k.is_empty() && !k.contains(PUBKEY_PLACEHOLDER))
}

/// Shown when a build has no update key.
pub fn not_configured() -> &'static str {
    tr!(
        "Automatische Updates sind in diesem Build nicht eingerichtet",
        "Automatic updates are not set up in this build"
    )
}

/// The release page of `version` (`1.2.0` or `v1.2.0`).
pub fn release_url(version: &str) -> String {
    let v = version.trim().trim_start_matches('v');
    format!("https://github.com/{REPOSITORY}/releases/tag/v{v}")
}

/// The page of the newest release (where a portable copy or a package is downloaded by hand).
pub fn latest_release_url() -> String {
    format!("https://github.com/{REPOSITORY}/releases/latest")
}

fn parse(version: &str) -> Option<semver::Version> {
    semver::Version::parse(version.trim().trim_start_matches('v')).ok()
}

fn same(a: &str, b: &str) -> bool {
    parse(a).is_some_and(|x| parse(b).is_some_and(|y| x.cmp_precedence(&y).is_eq()))
}

/// Whether `offered` (the feed's version) is an update for `current`: newer by semver precedence,
/// build metadata ignored. A release build is never moved to a pre-release; a pre-release build
/// takes a later pre-release or the final version. A version that does not parse is no update.
pub fn is_newer(current: &str, offered: &str) -> bool {
    let (Some(current), Some(offered)) = (parse(current), parse(offered)) else {
        return false;
    };
    if !offered.pre.is_empty() && current.pre.is_empty() {
        return false;
    }
    offered.cmp_precedence(&current).is_gt()
}

/// How updates arrive (Settings → Über).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    /// Downloaded in the background and installed when the app quits (or on „Jetzt neu starten“).
    #[default]
    Auto,
    /// Only a notice; downloaded and installed on the user's click.
    Notify,
}

/// Why this copy does not install updates itself (it offers the release page instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Manual {
    /// The portable ZIP (Windows): it would be installed into the user profile instead of its folder.
    Portable,
    /// Installed as .deb or .rpm: the package manager owns the program files.
    Package,
    /// macOS runs the app from a temporary read-only copy (App Translocation: started from the
    /// Downloads folder or a disk image without being moved into Applications).
    Translocated,
    /// The program is on a read-only volume (a mounted disk image, a write-protected drive).
    ReadOnly,
    /// No write permission for the folder of the AppImage.
    NotWritable,
    /// Linux: the program was not started as an AppImage (an unpacked AppImage, an own build).
    #[serde(rename = "no_appimage")]
    NoAppImage,
}

impl Manual {
    /// The name the UI and the test runs use (`portable`, `translocated`, …).
    pub fn parse(raw: &str) -> Option<Manual> {
        match raw.trim() {
            "portable" => Some(Manual::Portable),
            "package" => Some(Manual::Package),
            "translocated" => Some(Manual::Translocated),
            "read_only" => Some(Manual::ReadOnly),
            "not_writable" => Some(Manual::NotWritable),
            "no_appimage" => Some(Manual::NoAppImage),
            _ => None,
        }
    }

    /// In words a user can act on.
    pub fn message(self) -> &'static str {
        match self {
            Manual::Portable => tr!(
                "Die portable Version installiert Updates nicht selbst – lade die neue ZIP-Datei von der Release-Seite und entpacke sie über diesen Ordner",
                "The portable version does not install updates itself – download the new ZIP file from the release page and unpack it over this folder"
            ),
            Manual::Package => tr!(
                "Arcalo Mail ist als Paket installiert – lade das neue Paket von der Release-Seite oder aktualisiere es mit der Paketverwaltung",
                "Arcalo Mail is installed as a package – download the new package from the release page or update it with your package manager"
            ),
            Manual::Translocated => tr!(
                "macOS startet Arcalo Mail aus einem schreibgeschützten Zwischenordner, weil die App nicht im Ordner „Programme“ liegt – zieh Arcalo Mail nach „Programme“ und starte es von dort, dann installieren sich Updates selbst",
                "macOS runs Arcalo Mail from a read-only temporary folder because the app is not in the Applications folder – move Arcalo Mail to Applications and start it from there, then updates install themselves"
            ),
            Manual::ReadOnly => tr!(
                "Arcalo Mail liegt auf einem schreibgeschützten Laufwerk (etwa dem geöffneten Disk-Image) und kann sich nicht selbst aktualisieren – lade die neue Version von der Release-Seite",
                "Arcalo Mail is on a read-only drive (such as the opened disk image) and cannot update itself – download the new version from the release page"
            ),
            Manual::NotWritable => tr!(
                "Keine Schreibrechte für den Ordner der AppImage-Datei – lade die neue AppImage von der Release-Seite",
                "No write permission for the folder of the AppImage – download the new AppImage from the release page"
            ),
            Manual::NoAppImage => tr!(
                "Arcalo Mail wurde nicht als AppImage gestartet und kann sich nicht selbst aktualisieren – lade die neue Version von der Release-Seite",
                "Arcalo Mail was not started as an AppImage and cannot update itself – download the new version from the release page"
            ),
        }
    }
}

/// Whether the running copy can replace its own program files. `os` is `std::env::consts::OS`;
/// `program` the `.app` bundle (macOS) or the AppImage file (Linux; `None`: not started as an
/// AppImage); `write_probe` the error of creating a file next to it (`None`: that worked).
/// Windows installs through the NSIS setup into the user's own program folder: never blocked.
/// On macOS a folder without write permission is not blocking (the install asks for an
/// administrator's password); a read-only volume is.
pub fn install_blocked(os: &str, program: Option<&Path>, write_probe: Option<std::io::ErrorKind>) -> Option<Manual> {
    use std::io::ErrorKind;
    match (os, program) {
        ("macos", Some(bundle)) if bundle.components().any(|c| c.as_os_str() == "AppTranslocation") => {
            Some(Manual::Translocated)
        }
        ("macos", Some(bundle)) if bundle.starts_with("/Volumes") && write_probe.is_some() => Some(Manual::ReadOnly),
        ("macos", Some(_)) => (write_probe == Some(ErrorKind::ReadOnlyFilesystem)).then_some(Manual::ReadOnly),
        ("linux", None) => Some(Manual::NoAppImage),
        ("linux", Some(_)) => match write_probe {
            Some(ErrorKind::ReadOnlyFilesystem) => Some(Manual::ReadOnly),
            Some(ErrorKind::PermissionDenied) => Some(Manual::NotWritable),
            _ => None,
        },
        _ => None,
    }
}

/// Why this copy offers the release page instead of installing: a portable copy first (it is never
/// installed into the profile), then a package, then program files it cannot replace.
pub fn manual_reason(portable: bool, package: bool, blocked: Option<Manual>) -> Option<Manual> {
    if portable {
        Some(Manual::Portable)
    } else if package {
        Some(Manual::Package)
    } else {
        blocked
    }
}

/// A test run's stand-in (`ARCALO_MAIL_UPDATE_ENDPOINT`, `ARCALO_MAIL_UPDATE_PUBKEY`, …), honored
/// only by debug builds: a release build always asks the GitHub feed with its compiled-in key.
pub fn test_override(debug_build: bool, raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|v| debug_build && !v.is_empty())
}

/// Whole download percentage, `None` while the size is unknown.
pub fn progress_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|t| *t > 0)?;
    Some((downloaded.min(total) * 100 / total) as u8)
}

// ------------------------------------------------------------------ the first start after an update

/// File in the data folder written right before an update is installed; the next start reads it.
pub const RESTART_MARKER: &str = ".arcalo-mail-update";

/// The first start after an update was installed (or tried).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AfterUpdate {
    /// The version the update was to install.
    pub version: String,
    /// This start runs that version (otherwise the setup did not finish, e.g. it was cancelled).
    pub installed: bool,
    /// The version the update came from.
    pub from: Option<String>,
}

/// Notes that `version` is being installed over `from` (see [`take_restart_marker`]).
pub fn write_restart_marker(dir: &Path, version: &str, from: &str) -> std::io::Result<()> {
    std::fs::write(dir.join(RESTART_MARKER), format!("{}\nfrom={}", version.trim(), from.trim()))
}

/// Forgets the marker (the install failed and this process keeps running).
pub fn clear_restart_marker(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(RESTART_MARKER));
}

/// Reads and removes the marker at startup: whether this start follows an update.
pub fn take_restart_marker(dir: &Path, current: &str) -> Option<AfterUpdate> {
    let path = dir.join(RESTART_MARKER);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let version = lines.next().unwrap_or_default().trim().trim_start_matches('v').to_string();
    parse(&version)?;
    let from = lines
        .find_map(|l| l.trim().strip_prefix("from="))
        .map(|f| f.trim().trim_start_matches('v').to_string())
        .filter(|f| parse(f).is_some());
    Some(AfterUpdate { installed: same(current, &version), version, from })
}

// ------------------------------------------------------------------ what the updater remembers

/// Folder (in the data folder) with the downloads and the updater's state.
pub const UPDATES_DIR: &str = "updates";
const STATE_FILE: &str = "state.json";

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Kept outside the database, so it is there before the store opens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// „Diese Version überspringen“: not offered again (a newer one is).
    pub skipped: Option<String>,
    /// „Später erinnern“: no notice and no install before this time.
    pub remind_after: Option<DateTime<Utc>>,
    /// The version whose „Neu in Arcalo Mail“ was shown after its update.
    pub whats_new_seen: Option<String>,
    /// The release notes of the version being installed, shown by „Neu in Arcalo Mail“ after it.
    pub notes: Option<Notes>,
}

/// The release notes of a version, as the feed carried them (Markdown).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    pub version: String,
    pub text: String,
}

impl UpdateState {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(UPDATES_DIR).join(STATE_FILE)
    }

    pub fn load(data_dir: &Path) -> UpdateState {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        write_atomic(&Self::path(data_dir), &serde_json::to_string_pretty(self).unwrap_or_default())
    }

    /// The notes to show after the update to `version`, if they were kept for it.
    pub fn notes_for(&self, version: &str) -> Option<&str> {
        self.notes.as_ref().filter(|n| same(&n.version, version)).map(|n| n.text.as_str())
    }
}

/// What to do with the version a feed offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Offer,
    UpToDate,
    Skipped,
    /// „Später erinnern“ is still running.
    Snoozed,
}

/// Whether `offered` is offered to a copy running `current`.
pub fn verdict(current: &str, offered: &str, state: &UpdateState, now: DateTime<Utc>) -> Verdict {
    if !is_newer(current, offered) {
        return Verdict::UpToDate;
    }
    // The skipped version, and anything not newer than it.
    if let (Some(skip), Some(off)) = (state.skipped.as_deref().and_then(parse), parse(offered))
        && !off.cmp_precedence(&skip).is_gt()
    {
        return Verdict::Skipped;
    }
    if state.remind_after.is_some_and(|t| t > now) {
        return Verdict::Snoozed;
    }
    Verdict::Offer
}

/// „Neu in Arcalo Mail“ after an update: the release notes without the sections meant for the
/// people who test a release („## Checklist …“) and without the title the notes start with
/// („# Arcalo Mail 1.0.1“ or „## Arcalo Mail 1.0.1“: the dialog names the version itself).
pub fn notes_for_users(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in markdown.lines() {
        let heading = line.trim_start_matches('#');
        let level = line.len() - heading.len();
        if level > 0 && heading.starts_with(' ') {
            let title = heading.trim();
            if out.iter().all(|l: &&str| l.trim().is_empty()) && title.starts_with("Arcalo Mail") {
                continue;
            }
            if level == 2 {
                skipping = title.to_ascii_lowercase().starts_with("checklist");
            }
        }
        if !skipping {
            out.push(line);
        }
    }
    out.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_updater_needs_a_real_key() {
        assert_eq!(configured_pubkey(None), None);
        assert_eq!(configured_pubkey(Some("")), None);
        assert_eq!(configured_pubkey(Some("  \n")), None);
        assert_eq!(configured_pubkey(Some(PUBKEY_PLACEHOLDER)), None);
        assert_eq!(configured_pubkey(Some(" dW50cnVzdGVk\n")), Some("dW50cnVzdGVk"));
    }

    #[test]
    fn release_links_use_the_tag() {
        let url = "https://github.com/MouseWerk/arcalo-mail/releases/tag/v1.2.0";
        assert_eq!(release_url("1.2.0"), url);
        assert_eq!(release_url("v1.2.0"), url);
        assert!(FEED_URL.starts_with(&format!("https://github.com/{REPOSITORY}/")));
    }

    #[test]
    fn only_newer_versions_are_updates() {
        assert!(is_newer("1.0.0", "1.0.1"));
        assert!(is_newer("1.0.0", "v1.1.0"));
        assert!(is_newer("1.9.0", "1.10.0"), "numeric, not lexical");
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.1.0", "1.0.9"), "never a downgrade");
        assert!(!is_newer("1.0.0+win", "1.0.0+mac"), "build metadata is no new version");
        assert!(!is_newer("1.0.0", "garbage"));
        assert!(!is_newer("", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.1.0-beta.1"), "a release build stays on releases");
        assert!(is_newer("1.1.0-beta.1", "1.1.0-beta.2"));
        assert!(is_newer("1.1.0-beta.3", "1.1.0"), "the final version follows its pre-releases");
    }

    #[test]
    fn progress_is_clamped_and_unknown_without_size() {
        assert_eq!(progress_percent(10, None), None);
        assert_eq!(progress_percent(10, Some(0)), None);
        assert_eq!(progress_percent(101, Some(200)), Some(50));
        assert_eq!(progress_percent(300, Some(200)), Some(100));
    }

    #[test]
    fn copies_that_cannot_replace_themselves_say_why() {
        use std::io::ErrorKind::{Other, PermissionDenied, ReadOnlyFilesystem};
        let app = Path::new("/Applications/Arcalo Mail.app");
        assert_eq!(install_blocked("macos", Some(app), None), None);
        assert_eq!(install_blocked("macos", Some(app), Some(PermissionDenied)), None, "asks for the password");
        let translocated = Path::new("/private/var/folders/x/T/AppTranslocation/1F2E/d/Arcalo Mail.app");
        assert_eq!(install_blocked("macos", Some(translocated), Some(ReadOnlyFilesystem)), Some(Manual::Translocated));
        let dmg = Path::new("/Volumes/Arcalo Mail/Arcalo Mail.app");
        assert_eq!(install_blocked("macos", Some(dmg), Some(ReadOnlyFilesystem)), Some(Manual::ReadOnly));
        assert_eq!(install_blocked("macos", Some(dmg), Some(PermissionDenied)), Some(Manual::ReadOnly));
        assert_eq!(install_blocked("macos", None, None), None, "not in a bundle (a build of its own)");

        let appimage = Path::new("/home/u/Apps/ArcaloMail_1.0.0_amd64.AppImage");
        assert_eq!(install_blocked("linux", Some(appimage), None), None);
        assert_eq!(install_blocked("linux", Some(appimage), Some(PermissionDenied)), Some(Manual::NotWritable));
        assert_eq!(install_blocked("linux", Some(appimage), Some(ReadOnlyFilesystem)), Some(Manual::ReadOnly));
        assert_eq!(install_blocked("linux", Some(appimage), Some(Other)), None, "tried, and explained if it fails");
        assert_eq!(install_blocked("linux", None, None), Some(Manual::NoAppImage));
        assert_eq!(install_blocked("windows", None, Some(PermissionDenied)), None);
    }

    #[test]
    fn portable_and_packaged_copies_do_not_install() {
        assert_eq!(manual_reason(false, false, None), None);
        assert_eq!(manual_reason(true, true, Some(Manual::ReadOnly)), Some(Manual::Portable));
        assert_eq!(manual_reason(false, true, Some(Manual::NoAppImage)), Some(Manual::Package));
        assert_eq!(manual_reason(false, false, Some(Manual::NotWritable)), Some(Manual::NotWritable));
        for m in [
            Manual::Portable,
            Manual::Package,
            Manual::Translocated,
            Manual::ReadOnly,
            Manual::NotWritable,
            Manual::NoAppImage,
        ] {
            let name = serde_json::to_value(m).unwrap();
            assert_eq!(Manual::parse(name.as_str().unwrap()), Some(m));
            assert!(!m.message().is_empty());
        }
        assert_eq!(Manual::parse("deb"), None);
    }

    #[test]
    fn test_feed_only_in_debug_builds() {
        let url = "http://127.0.0.1:8123/latest.json";
        assert_eq!(test_override(true, Some(url)), Some(url));
        assert_eq!(test_override(false, Some(url)), None, "release builds cannot be redirected");
        assert_eq!(test_override(true, Some("  ")), None);
        assert_eq!(test_override(true, None), None);
    }

    #[test]
    fn the_restart_marker_is_read_once() {
        let dir = tmp("marker");
        assert_eq!(take_restart_marker(&dir, "1.0.1"), None);
        write_restart_marker(&dir, "1.0.1", "1.0.0").unwrap();
        assert_eq!(
            take_restart_marker(&dir, "1.0.1"),
            Some(AfterUpdate { version: "1.0.1".into(), installed: true, from: Some("1.0.0".into()) })
        );
        assert_eq!(take_restart_marker(&dir, "1.0.1"), None, "removed after reading");
        // The setup did not finish: the old version starts again.
        write_restart_marker(&dir, "1.0.1", "1.0.0").unwrap();
        assert!(!take_restart_marker(&dir, "1.0.0").unwrap().installed);
        write_restart_marker(&dir, "1.0.1", "1.0.0").unwrap();
        clear_restart_marker(&dir);
        assert_eq!(take_restart_marker(&dir, "1.0.1"), None);
        std::fs::write(dir.join(RESTART_MARKER), "not a version").unwrap();
        assert_eq!(take_restart_marker(&dir, "1.0.1"), None);
        assert!(!dir.join(RESTART_MARKER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skipped_and_snoozed_versions_wait() {
        let now = Utc::now();
        let mut s = UpdateState::default();
        assert_eq!(verdict("1.0.0", "1.0.1", &s, now), Verdict::Offer);
        assert_eq!(verdict("1.0.1", "1.0.1", &s, now), Verdict::UpToDate);
        s.skipped = Some("1.0.1".into());
        assert_eq!(verdict("1.0.0", "1.0.1", &s, now), Verdict::Skipped);
        assert_eq!(verdict("1.0.0", "1.0.2", &s, now), Verdict::Offer, "the next newer version shows again");
        s.skipped = None;
        s.remind_after = Some(now + Duration::days(1));
        assert_eq!(verdict("1.0.0", "1.0.1", &s, now), Verdict::Snoozed);
        assert_eq!(verdict("1.0.0", "1.0.1", &s, now + Duration::days(2)), Verdict::Offer, "tomorrow is over");
    }

    #[test]
    fn the_state_survives_a_restart() {
        let dir = tmp("state");
        assert_eq!(UpdateState::load(&dir), UpdateState::default());
        let s = UpdateState {
            skipped: Some("1.2.0".into()),
            notes: Some(Notes { version: "1.0.1".into(), text: "- Faster".into() }),
            ..Default::default()
        };
        s.save(&dir).unwrap();
        let back = UpdateState::load(&dir);
        assert_eq!(back, s);
        assert_eq!(back.notes_for("1.0.1"), Some("- Faster"));
        assert_eq!(back.notes_for("1.0.2"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn users_see_the_notes_without_the_checklist() {
        let notes = "# Arcalo Mail 1.0.1\n\n## Improved\n\n- Search is faster.\n\n## Checklist for Windows (manual)\n\n- [ ] Install\n\n## Fixed\n\n- A crash.\n";
        assert_eq!(notes_for_users(notes), "## Improved\n\n- Search is faster.\n\n## Fixed\n\n- A crash.");
        let first = "## Arcalo Mail 1.0.0\n\nThe first release.\n\n### Highlights\n\n- Calm.\n\n## Checklist (manual)\n\n### Windows\n\n- [ ] Install";
        assert_eq!(notes_for_users(first), "The first release.\n\n### Highlights\n\n- Calm.");
    }
}
