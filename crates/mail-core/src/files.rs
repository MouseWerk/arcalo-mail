//! Attachments on disk: a name that is safe to write and shows what it is, how risky a file is
//! to open, and the mark that tells the system a file came from the internet (Windows'
//! Mark-of-the-Web, macOS' quarantine), so SmartScreen, Office's Protected View and Gatekeeper
//! check it as they would a download.

use std::path::Path;

use serde::Serialize;

/// Bidirectional controls: they make `Rechnung\u{202E}fdp.exe` look like `Rechnungexe.pdf`.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A file name as the list shows it: without bidirectional and other invisible format
/// characters, so the real extension is what the user reads.
pub fn display_name(name: &str) -> String {
    name.chars().filter(|c| !is_bidi_control(*c) && !matches!(c, '\u{200B}'..='\u{200D}' | '\u{FEFF}')).collect()
}

/// A name that is safe to write in a folder: no path or reserved characters, no control or
/// bidirectional characters, no trailing dots or spaces (Windows drops them, so `a.exe.` would
/// become `a.exe`), never empty or hidden.
pub fn safe_name(name: &str) -> String {
    let n: String =
        display_name(name).chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let n = n.trim_end_matches(['.', ' ']).trim_start().to_string();
    if n.is_empty() || n.starts_with('.') { format!("attachment{n}") } else { n }
}

/// How risky a file is to open from a mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    None,
    /// An Office document with macros: opened only after a warning.
    Macro,
    /// A program, script or shortcut: never opened from the app, only saved.
    Blocked,
}

/// Extensions that run code when opened (programs, installers, scripts, shortcuts), after
/// Outlook's level-1 list.
const BLOCKED: &[&str] = &[
    "ade",
    "adp",
    "app",
    "appimage",
    "application",
    "appref-ms",
    "apk",
    "asp",
    "bas",
    "bat",
    "cer",
    "chm",
    "cmd",
    "cnt",
    "com",
    "command",
    "cpl",
    "crt",
    "csh",
    "der",
    "desktop",
    "diagcab",
    "exe",
    "fxp",
    "gadget",
    "grp",
    "hlp",
    "hpj",
    "hta",
    "htc",
    "inf",
    "ins",
    "isp",
    "its",
    "jar",
    "jnlp",
    "js",
    "jse",
    "ksh",
    "lnk",
    "library-ms",
    "mad",
    "maf",
    "mag",
    "mam",
    "maq",
    "mar",
    "mas",
    "mat",
    "mau",
    "mav",
    "maw",
    "mcf",
    "mda",
    "mdb",
    "mde",
    "mdt",
    "mdw",
    "mdz",
    "msc",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "mst",
    "msu",
    "ops",
    "osd",
    "pcd",
    "pif",
    "pkg",
    "pl",
    "plg",
    "prf",
    "prg",
    "printerexport",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psd1",
    "psdm1",
    "py",
    "pyc",
    "pyo",
    "pyw",
    "pyz",
    "pyzw",
    "reg",
    "run",
    "scf",
    "scpt",
    "scr",
    "sct",
    "search-ms",
    "settingcontent-ms",
    "sh",
    "shb",
    "shs",
    "terminal",
    "theme",
    "url",
    "vb",
    "vbe",
    "vbp",
    "vbs",
    "vhd",
    "vhdx",
    "vsmacros",
    "vsw",
    "webloc",
    "webpnp",
    "website",
    "workflow",
    "ws",
    "wsb",
    "wsc",
    "wsf",
    "wsh",
    "xbap",
    "xll",
    "xnk",
    "iso",
    "img",
];

/// Office documents that carry macros.
const MACRO: &[&str] =
    &["docm", "dotm", "xlsm", "xltm", "xlam", "xlsb", "pptm", "potm", "ppam", "ppsm", "sldm", "accde", "one"];

/// How risky `name` (as it would be written, [`safe_name`]) is to open.
pub fn risk(name: &str) -> Risk {
    let n = safe_name(name).to_ascii_lowercase();
    let Some((_, ext)) = n.rsplit_once('.') else { return Risk::None };
    if BLOCKED.contains(&ext) {
        Risk::Blocked
    } else if MACRO.contains(&ext) {
        Risk::Macro
    } else {
        Risk::None
    }
}

/// Marks a file written from a mail as coming from the internet: on Windows the
/// `Zone.Identifier` stream (zone 3), on macOS the `com.apple.quarantine` attribute. Elsewhere
/// there is no such mark. Errors are ignored: a file system without streams (FAT, a network
/// share) still gets the file.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(&stream, zone_identifier());
    }
    #[cfg(target_os = "macos")]
    {
        let _ = set_quarantine(path);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
}

/// The content of the `Zone.Identifier` stream: the internet zone.
pub fn zone_identifier() -> &'static str {
    "[ZoneTransfer]\r\nZoneId=3\r\n"
}

/// The value of `com.apple.quarantine`: flags (downloaded, checked by Gatekeeper), the time in
/// hex, the agent.
pub fn quarantine_value(at: i64) -> String {
    format!("0083;{at:x};Arcalo Mail;")
}

#[cfg(target_os = "macos")]
fn set_quarantine(path: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn setxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *const std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
    }
    let p = CString::new(path.as_os_str().as_bytes())?;
    let name = CString::new("com.apple.quarantine")?;
    let value = quarantine_value(crate::store::now());
    // SAFETY: both strings are NUL-terminated and live for the call; the value is a byte slice
    // with its length.
    let r = unsafe { setxattr(p.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
    if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Writes an attachment where it goes and marks it as coming from the internet.
pub fn write_marked(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)?;
    mark_from_internet(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_bidi_controls_and_trailing_dots() {
        assert_eq!(safe_name("Rechnung_2026\u{202E}fdp.exe"), "Rechnung_2026fdp.exe");
        assert_eq!(display_name("Rechnung_2026\u{202E}fdp.exe"), "Rechnung_2026fdp.exe");
        assert_eq!(safe_name("a\u{2067}b\u{200F}.pdf"), "ab.pdf");
        assert_eq!(safe_name("invoice.exe. . "), "invoice.exe");
        assert_eq!(safe_name("../../x.lnk"), "attachment.._.._x.lnk");
        assert_eq!(safe_name("..."), "attachment");
        assert_eq!(safe_name(".bashrc"), "attachment.bashrc");
        assert_eq!(safe_name("a\u{0}b.txt"), "a_b.txt");
    }

    #[test]
    fn programs_scripts_and_shortcuts_are_blocked() {
        for n in [
            "a.exe",
            "b.MSI",
            "c.bat",
            "d.cmd",
            "e.ps1",
            "f.vbs",
            "g.js",
            "h.jse",
            "i.wsf",
            "j.scr",
            "k.com",
            "l.lnk",
            "m.url",
            "n.hta",
            "o.jar",
            "p.app",
            "q.command",
            "r.sh",
            "s.desktop",
            "t.AppImage",
            "u.apk",
            "invoice.pdf.lnk",
            "x\u{202E}fdp.exe",
            "y.exe.",
            "z.exe ",
        ] {
            assert_eq!(risk(n), Risk::Blocked, "{n}");
        }
        for n in ["a.docm", "b.xlsm", "c.pptm"] {
            assert_eq!(risk(n), Risk::Macro, "{n}");
        }
        for n in ["a.pdf", "b.docx", "c.jpg", "readme", "d.txt", "e.ics"] {
            assert_eq!(risk(n), Risk::None, "{n}");
        }
    }

    #[test]
    fn the_marks_say_internet() {
        assert_eq!(zone_identifier(), "[ZoneTransfer]\r\nZoneId=3\r\n");
        assert_eq!(quarantine_value(0x1234), "0083;1234;Arcalo Mail;");
    }
}
