//! Arcalo Mail as the system's mail app: e-mail links (`mailto:`) of other apps open the compose
//! sheet (`mail_core::mailto`), and Settings → Über offers „Als Standard-Mail-App festlegen“
//! where the system lets an app ask for it.
//!
//! - Windows: the setup registers the app as a mail client (`installer/hooks.nsh`: the
//!   `ArcaloMail.mailto` class, `Clients\Mail` and `RegisteredApplications`); Windows lets only
//!   the user choose the default, so the button opens Settings → Apps → Default apps on the app.
//! - macOS: the bundle declares `mailto` (`Info.plist`); the button asks Launch Services to make
//!   the app the handler, macOS confirms it with the user.
//! - Linux: the .deb and .rpm install a desktop entry with `MimeType=x-scheme-handler/mailto`;
//!   the button makes it the default through `xdg-mime`. An AppImage (or a copy started from
//!   elsewhere) first writes a desktop entry of its own under `~/.local/share/applications`.
//! - Android: the app takes `mailto:` links (`AndroidManifest.xml`); Android asks which app
//!   opens the first one, so the settings only say so. iOS lets only apps with Apple's mail
//!   entitlement become the default: not offered.
//!
//! The address reaches the app on the command line (Windows, Linux: the first start, or the
//! single-instance plugin for a running app), through the system's open event (macOS) or the
//! activity's intent (Android); [`open`] hands it to the UI (`compose-mailto`), and a UI still
//! starting takes it with [`take_pending_mailto`].

use std::sync::Mutex;

use mail_core::mailto::Mailto;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

static PENDING: Mutex<Option<Mailto>> = Mutex::new(None);

/// A `mailto:` address asked the app to write a message.
pub fn open(app: Option<&AppHandle>, m: Mailto) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.clone());
    if let Some(app) = app {
        #[cfg(desktop)]
        crate::tray::show_main(app);
        let _ = app.emit("compose-mailto", ());
    }
}

/// The `mailto:` address among the arguments of a start (or of a second start, which the
/// single-instance plugin hands to the running app).
pub fn open_from_args(app: Option<&AppHandle>, args: &[String]) {
    if let Some(m) = mail_core::mailto::from_args(args) {
        open(app, m);
    }
}

/// The message a `mailto:` address asked for, once.
#[tauri::command]
pub fn take_pending_mailto() -> Option<Mailto> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// What Settings → Über shows.
#[derive(Debug, Clone, Serialize)]
pub struct DefaultMail {
    /// The button is offered on this system.
    pub supported: bool,
    /// Arcalo Mail opens `mailto:` links now (`None`: the system does not say).
    pub is_default: Option<bool>,
    /// What the button does: `set` (the app makes itself the default, the system may confirm) or
    /// `settings` (the system's settings open, the user chooses there).
    pub how: &'static str,
    /// Why it is not offered, or what to do instead (Android).
    pub note: Option<String>,
}

#[tauri::command]
pub fn default_mail_status(app: AppHandle) -> DefaultMail {
    platform::status(&app)
}

/// „Als Standard-Mail-App festlegen“.
#[tauri::command]
pub fn default_mail_set(app: AppHandle) -> Result<DefaultMail, mail_core::Error> {
    platform::set(&app).map_err(mail_core::Error::Invalid)?;
    Ok(platform::status(&app))
}

#[cfg(target_os = "linux")]
mod platform {
    use super::DefaultMail;
    use std::path::PathBuf;
    use std::process::Command;
    use tauri::{AppHandle, Manager};

    const SCHEME: &str = "x-scheme-handler/mailto";
    /// The desktop entry an AppImage (or another copy without a package) writes for itself.
    const OWN_ENTRY: &str = "de.mousewerk.arcalo.mail.desktop";

    /// The desktop entry of the .deb and .rpm (the bundler names it after the product).
    fn packaged_entry(app: &AppHandle) -> Option<String> {
        use tauri::utils::config::BundleType;
        matches!(tauri::utils::platform::bundle_type(), Some(BundleType::Deb | BundleType::Rpm))
            .then(|| format!("{}.desktop", app.package_info().name))
    }

    fn applications_dir() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("applications"))
    }

    /// The desktop entry for a copy without a package: the program it runs is the AppImage file
    /// (or the binary of a build of its own).
    pub fn own_entry(program: &str, name: &str) -> String {
        let quoted = format!("\"{}\"", program.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$"));
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nComment=Local-first e-mail\nExec={quoted} %u\nIcon=mail-client\nTerminal=false\nCategories=Network;Email;Office;\nMimeType={SCHEME};\nNoDisplay=true\n"
        )
    }

    /// The desktop entry that opens `mailto:` links now (empty: none); `None` without `xdg-mime`.
    fn current() -> Option<String> {
        let out = Command::new("xdg-mime").args(["query", "default", SCHEME]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn status(app: &AppHandle) -> DefaultMail {
        let mine = packaged_entry(app).unwrap_or_else(|| OWN_ENTRY.to_string());
        let is_default = current().map(|id| id == mine);
        DefaultMail { supported: is_default.is_some(), is_default, how: "set", note: None }
    }

    pub fn set(app: &AppHandle) -> Result<(), String> {
        let entry = match packaged_entry(app) {
            Some(e) => e,
            None => {
                let program = app
                    .env()
                    .appimage
                    .map(PathBuf::from)
                    .or_else(|| std::env::current_exe().ok())
                    .ok_or_else(|| "no program path".to_string())?;
                let dir = applications_dir().ok_or_else(|| "no applications folder".to_string())?;
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let text = own_entry(&program.to_string_lossy(), &app.package_info().name);
                std::fs::write(dir.join(OWN_ENTRY), text).map_err(|e| e.to_string())?;
                let _ = Command::new("update-desktop-database").arg(&dir).status();
                OWN_ENTRY.to_string()
            }
        };
        let status = Command::new("xdg-mime").args(["default", &entry, SCHEME]).status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(mail_core::tr!(
                "Das System hat die Standard-Mail-App nicht geändert (xdg-mime)",
                "The system did not change the default mail app (xdg-mime)"
            )
            .into());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_own_desktop_entry_takes_mailto_links() {
            let e = super::own_entry("/home/u/Apps/Arcalo Mail $1.AppImage", "Arcalo Mail");
            assert!(e.contains("Exec=\"/home/u/Apps/Arcalo Mail \\$1.AppImage\" %u\n"), "{e}");
            assert!(e.contains("MimeType=x-scheme-handler/mailto;\n"));
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::DefaultMail;
    use tauri::AppHandle;

    /// The class the setup registers for `mailto:` (`installer/hooks.nsh`).
    const PROG_ID: &str = "ArcaloMail.mailto";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The class the user chose for `mailto:` in Windows' settings.
    fn user_choice() -> Option<String> {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
        let key = wide(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\mailto\UserChoice");
        let value = wide("ProgId");
        let mut buf = [0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: the strings are NUL-terminated, the buffer and its size in bytes match.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn status(_app: &AppHandle) -> DefaultMail {
        if crate::portable::active() {
            return DefaultMail {
                supported: false,
                is_default: None,
                how: "settings",
                note: Some(
                    mail_core::tr!(
                        "Die portable Version meldet sich nicht bei Windows an; installiere Arcalo Mail, um es als Standard-Mail-App festzulegen",
                        "The portable version does not register with Windows; install Arcalo Mail to make it the default mail app"
                    )
                    .into(),
                ),
            };
        }
        let is_default = user_choice().map(|p| p == PROG_ID).unwrap_or(false);
        DefaultMail { supported: true, is_default: Some(is_default), how: "settings", note: None }
    }

    pub fn set(app: &AppHandle) -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        let name = app.package_info().name.replace(' ', "%20");
        app.opener()
            .open_url(format!("ms-settings:defaultapps?registeredAppUser={name}"), None::<&str>)
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::DefaultMail;
    use std::ffi::c_void;
    use tauri::AppHandle;

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: u8,
        ) -> CFStringRef;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultHandlerForURLScheme(scheme: CFStringRef) -> CFStringRef;
        fn LSSetDefaultHandlerForURLScheme(scheme: CFStringRef, handler: CFStringRef) -> i32;
    }

    /// A CFString owned by this value.
    struct Cf(CFStringRef);

    impl Cf {
        fn new(s: &str) -> Option<Cf> {
            // SAFETY: the bytes are valid UTF-8 of the given length; no allocator (the default).
            let r = unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) };
            (!r.is_null()).then_some(Cf(r))
        }

        fn text(&self) -> Option<String> {
            let mut buf = [0u8; 512];
            // SAFETY: the buffer and its length match; the string is alive while `self` is.
            let ok = unsafe { CFStringGetCString(self.0, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
            if ok == 0 {
                return None;
            }
            let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            Some(String::from_utf8_lossy(&buf[..len]).into_owned())
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: created or copied (owned) by this value, released once.
            unsafe { CFRelease(self.0) }
        }
    }

    fn current() -> Option<String> {
        let scheme = Cf::new("mailto")?;
        // SAFETY: a valid CFString; the result follows the copy rule (owned, released by Cf).
        let handler = unsafe { LSCopyDefaultHandlerForURLScheme(scheme.0) };
        if handler.is_null() {
            return None;
        }
        Cf(handler).text()
    }

    pub fn status(app: &AppHandle) -> DefaultMail {
        let id = &app.config().identifier;
        let is_default = current().map(|h| h.eq_ignore_ascii_case(id)).unwrap_or(false);
        DefaultMail { supported: true, is_default: Some(is_default), how: "set", note: None }
    }

    pub fn set(app: &AppHandle) -> Result<(), String> {
        let scheme = Cf::new("mailto").ok_or("mailto")?;
        let handler = Cf::new(&app.config().identifier).ok_or("identifier")?;
        // SAFETY: two valid CFStrings, alive for the call.
        let rc = unsafe { LSSetDefaultHandlerForURLScheme(scheme.0, handler.0) };
        if rc != 0 {
            return Err(format!("Launch Services: {rc}"));
        }
        Ok(())
    }
}

#[cfg(mobile)]
mod platform {
    use super::DefaultMail;
    use tauri::AppHandle;

    pub fn status(_app: &AppHandle) -> DefaultMail {
        let note = if cfg!(target_os = "android") {
            Some(
                mail_core::tr!(
                    "Tippe auf einen E-Mail-Link und wähle Arcalo Mail und „Immer“: dann öffnet Android E-Mail-Links hier",
                    "Tap an e-mail link and choose Arcalo Mail and “Always”: Android then opens e-mail links here"
                )
                .into(),
            )
        } else {
            None
        };
        DefaultMail { supported: false, is_default: None, how: "settings", note }
    }

    pub fn set(_app: &AppHandle) -> Result<(), String> {
        Err(mail_core::tr!("Auf diesem Gerät nicht möglich", "Not possible on this device").into())
    }
}
