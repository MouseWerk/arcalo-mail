//! Portable mode (the Windows ZIP of each release, as Arcalo's): a file `arcalo-mail-portable`
//! next to `Arcalo Mail.exe` keeps the database, the settings and the update state in `data`
//! beside the program, and the WebView's profile in `data\webview`. Updates are downloaded by
//! hand from the release page, never installed into the user profile. Passwords and sign-in
//! tokens stay in the computer's credential store: on another computer an account asks to sign
//! in again.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The marker file next to the program.
pub const MARKER: &str = "arcalo-mail-portable";

/// The data folder of a portable copy: `data` next to a program folder that holds the marker.
pub fn data_dir_for(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir.join(MARKER).is_file().then(|| exe_dir.join("data"))
}

/// Looked for once at startup; the portable data folder, or `None` for an installed copy.
pub fn detect() -> Option<PathBuf> {
    static PORTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    PORTABLE
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            data_dir_for(exe.parent()?)
        })
        .clone()
}

/// This copy runs portable.
pub fn active() -> bool {
    detect().is_some()
}

/// Before the WebView starts: its profile (cookies, caches) goes into the portable data folder
/// too, so the copy writes nothing into the user profile it runs on (WebView2 reads the
/// variable; the other platforms have no portable build).
pub fn prepare() {
    #[cfg(windows)]
    if let Some(dir) = detect() {
        // SAFETY: called at the start of `main`, before any other thread exists.
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("webview")) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_next_to_the_program_makes_it_portable() {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(data_dir_for(&dir), None);
        std::fs::write(dir.join(MARKER), "Arcalo Mail portable\r\n").unwrap();
        assert_eq!(data_dir_for(&dir), Some(dir.join("data")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
