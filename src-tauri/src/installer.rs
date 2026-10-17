//! Installs an update file that `updates.rs` downloaded and verified against the built-in key, as
//! Arcalo's `installer.rs` does (what the updater plugin does for these bundles, without its own
//! feed check and without any network):
//!
//! - Windows (NSIS): the setup is written to `%TEMP%\<app>-<version>-updater-XXXXXX\<app>-<version>-installer.exe`
//!   and started with ShellExecute and the plugin's arguments (`installMode` "passive": `/P /UPDATE`,
//!   plus `/R /ARGS <args>` to start the app again), then this process ends.
//! - macOS (`.app.tar.gz`): unpacked into a temp folder, the running bundle is moved aside and the
//!   new one moved into its place; without write access the move runs with administrator rights.
//! - Linux (AppImage): the AppImage file is replaced in place (the old one moved aside in a temp
//!   folder on the same file system first, and moved back if writing fails).

#[cfg(unix)]
use std::io::Cursor;
use std::path::{Path, PathBuf};

use mail_core::{tr, trf};

/// Why an update could not be installed.
#[derive(Debug)]
pub enum InstallError {
    Io(std::io::Error),
    /// The file is not what this platform installs (no setup, no archive).
    UnknownFormat,
    /// The archive holds no program of the expected kind.
    #[cfg_attr(windows, allow(dead_code))]
    NotInArchive,
    /// No temp folder on the program's file system (AppImage).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    NoTempDir,
    /// The setup could not be started (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Launch(std::io::Error),
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

impl InstallError {
    /// In words a user can act on.
    pub fn message(&self) -> String {
        match self {
            InstallError::Io(e) if e.kind() == std::io::ErrorKind::StorageFull => {
                tr!("Nicht genug freier Speicherplatz", "Not enough free disk space").into()
            }
            InstallError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                trf!(
                    "Keine Schreibrechte für den Programmordner ({e})",
                    "No write permission for the program folder ({e})"
                )
            }
            InstallError::Io(e) => trf!("Dateifehler ({e})", "File error ({e})"),
            InstallError::UnknownFormat => tr!(
                "Die Update-Datei ist kein Installationspaket für dieses System",
                "The update file is not an installation package for this system"
            )
            .into(),
            InstallError::NotInArchive => {
                tr!("Im Update-Paket fehlt das Programm", "The update package does not contain the program").into()
            }
            InstallError::NoTempDir => tr!(
                "Kein Ordner für temporäre Dateien auf demselben Laufwerk wie das Programm",
                "No folder for temporary files on the same drive as the program"
            )
            .into(),
            InstallError::Launch(e) => trf!(
                "Das Installationsprogramm konnte nicht gestartet werden ({e})",
                "The installer could not be started ({e})"
            ),
        }
    }
}

type Result<T> = std::result::Result<T, InstallError>;

/// The Windows install mode (`plugins.updater.windows.installMode` in `tauri.conf.json`), with
/// the NSIS arguments the updater plugin gives each one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallMode {
    BasicUi,
    Quiet,
    #[default]
    Passive,
}

impl InstallMode {
    fn nsis_args(self) -> &'static [&'static str] {
        match self {
            InstallMode::Passive => &["/P"],
            InstallMode::Quiet => &["/S"],
            InstallMode::BasicUi => &[],
        }
    }

    fn nsis_restart_args(self) -> &'static [&'static str] {
        match self {
            InstallMode::BasicUi => &[],
            _ => &["/R"],
        }
    }
}

/// Install mode and extra installer arguments from the `plugins.updater` section of the config.
pub fn windows_config(updater: Option<&serde_json::Value>) -> (InstallMode, Vec<String>) {
    let windows = updater.and_then(|u| u.get("windows"));
    let mode = match windows.and_then(|w| w.get("installMode")).and_then(|m| m.as_str()) {
        Some("basicUi") => InstallMode::BasicUi,
        Some("quiet") => InstallMode::Quiet,
        _ => InstallMode::Passive,
    };
    let args = windows
        .and_then(|w| w.get("installerArgs"))
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
        .unwrap_or_default();
    (mode, args)
}

/// What the install needs to know about the running copy.
#[derive(Clone, Debug)]
pub struct Target {
    /// The product name (`Arcalo Mail`): part of the installer's temp file name.
    pub app_name: String,
    /// The version being installed.
    pub version: String,
    /// The running program: the `.app`'s binary on macOS, the AppImage on Linux.
    #[cfg_attr(windows, allow(dead_code))]
    pub exe: PathBuf,
    /// The arguments the restarted app gets (Windows installer).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub app_args: Vec<String>,
    /// The Windows installer starts the new version.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub restart: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub mode: InstallMode,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub installer_args: Vec<String>,
}

impl Target {
    /// The running copy. `restart`: the Windows installer starts the new version afterwards.
    pub fn current(app: &tauri::AppHandle, version: &str, restart: bool) -> Result<Self> {
        use tauri::Manager;
        let (mode, installer_args) = windows_config(app.config().plugins.0.get("updater"));
        let env = app.env();
        #[cfg(target_os = "linux")]
        let exe = match env.appimage.clone() {
            Some(p) => PathBuf::from(p),
            None => std::env::current_exe()?,
        };
        #[cfg(not(target_os = "linux"))]
        let exe = std::env::current_exe()?;
        // A `mailto:` address the app was started with is not opened again after the update.
        let app_args = env
            .args_os
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .filter(|a| mail_core::mailto::parse(a).is_none())
            .collect();
        Ok(Target {
            app_name: app.package_info().name.clone(),
            version: version.to_string(),
            exe,
            app_args,
            restart,
            mode,
            installer_args,
        })
    }
}

/// Installs `bytes` (already verified). Windows: `before_exit` runs once the installer is
/// written, right before it is started; a started installer ends this process. Elsewhere the
/// new version is in place when this returns (the caller restarts) and `before_exit` is not used.
#[cfg_attr(not(windows), allow(unused_variables))]
pub fn install(bytes: &[u8], target: &Target, before_exit: impl FnOnce()) -> Result<()> {
    #[cfg(windows)]
    {
        let plan = prepare_nsis(bytes, target, &std::env::temp_dir())?;
        before_exit();
        if let Err(e) = launch(&plan) {
            if let Some(dir) = plan.file.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(InstallError::Launch(e));
        }
        std::process::exit(0);
    }
    #[cfg(target_os = "macos")]
    {
        replace_bundle(bytes, &bundle_of(&target.exe).ok_or(InstallError::UnknownFormat)?)
    }
    #[cfg(target_os = "linux")]
    {
        replace_appimage(bytes, &target.exe)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(InstallError::UnknownFormat)
    }
}

/// Whether the running copy can replace its own program files (`mail_core::update::install_blocked`):
/// the bundle or AppImage it runs from, and whether a file can be created next to it.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn blocked(app: &tauri::AppHandle) -> Option<mail_core::update::Manual> {
    #[cfg(target_os = "linux")]
    let program = {
        use tauri::Manager;
        app.env().appimage.map(PathBuf::from)
    };
    #[cfg(target_os = "macos")]
    let program = std::env::current_exe().ok().and_then(|exe| bundle_of(&exe));
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let program: Option<PathBuf> = None;
    let probe = program.as_deref().and_then(Path::parent).and_then(write_probe);
    mail_core::update::install_blocked(std::env::consts::OS, program.as_deref(), probe)
}

/// Creates and removes a file in `dir`: the kind of error when that is not possible.
pub fn write_probe(dir: &Path) -> Option<std::io::ErrorKind> {
    tempfile::Builder::new().prefix(".arcalo-mail-update-check").tempfile_in(dir).err().map(|e| e.kind())
}

// ------------------------------------------------------------------ Windows

/// The installer to start and its command line.
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct WindowsPlan {
    pub file: PathBuf,
    pub parameters: String,
}

/// `<app>-<version>-updater-`: prefix of the temp folder the installer is written to.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn temp_dir_prefix(app_name: &str, version: &str) -> String {
    format!("{app_name}-{version}-updater-")
}

/// `<app>-<version>-installer.exe`: the installer's file name in that folder.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn installer_file_name(app_name: &str, version: &str) -> String {
    format!("{app_name}-{version}-installer.exe")
}

/// Quotes an argument for the NSIS command line: like Windows' own quoting, and `/` is quoted
/// too so NSIS does not read it as one of its options.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn escape_nsis_arg(arg: &str) -> String {
    let quote = arg.is_empty() || arg.contains([' ', '\t', '/']);
    let mut out = String::with_capacity(arg.len() + 2);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    out
}

/// The NSIS installer's command line: the mode, `/UPDATE`, and with `restart` the switches that
/// start the app again with its arguments.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn nsis_parameters(mode: InstallMode, restart: bool, app_args: &[String], installer_args: &[String]) -> String {
    let mut args: Vec<String> = mode.nsis_args().iter().map(|a| a.to_string()).collect();
    args.push("/UPDATE".into());
    if restart {
        args.extend(mode.nsis_restart_args().iter().map(|a| a.to_string()));
        args.push("/ARGS".into());
        args.extend(app_args.iter().map(|a| escape_nsis_arg(a)));
    }
    args.extend(installer_args.iter().cloned());
    args.join(" ")
}

/// Writes the setup into a new folder under `temp_root` (kept after this process ends, the setup
/// runs from it) and builds its command line.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn prepare_nsis(bytes: &[u8], target: &Target, temp_root: &Path) -> Result<WindowsPlan> {
    if !bytes.starts_with(b"MZ") {
        return Err(InstallError::UnknownFormat);
    }
    let dir = tempfile::Builder::new()
        .prefix(&temp_dir_prefix(&target.app_name, &target.version))
        .tempdir_in(temp_root)?
        .keep();
    let file = dir.join(installer_file_name(&target.app_name, &target.version));
    if let Err(e) = std::fs::write(&file, bytes) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e.into());
    }
    let parameters = nsis_parameters(target.mode, target.restart, &target.app_args, &target.installer_args);
    Ok(WindowsPlan { file, parameters })
}

/// Starts the installer the way the plugin does (ShellExecute "open").
#[cfg(windows)]
fn launch(plan: &WindowsPlan) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
    let verb = wide(OsStr::new("open"));
    let file = wide(plan.file.as_os_str());
    let parameters = wide(OsStr::new(&plan.parameters));
    mail_core::diag::log("update", &format!("starting {} {}", plan.file.display(), plan.parameters));
    // SAFETY: every pointer is a NUL-terminated UTF-16 buffer alive for the call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOW,
        )
    };
    if result as isize <= 32 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

// ------------------------------------------------------------------ macOS

/// The `.app` folder of a binary in `<bundle>.app/Contents/MacOS/`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    contents.parent().map(Path::to_path_buf)
}

/// The AppleScript that replaces the bundle with administrator rights (paths quoted for the
/// shell, then for AppleScript).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn admin_move_script(bundle: &Path, new: &Path) -> String {
    let sh = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', r"'\''"));
    let command = format!("rm -rf {} && mv -f {} {}", sh(bundle), sh(new), sh(bundle));
    let applescript = command.replace('\\', r"\\").replace('"', "\\\"");
    format!("do shell script \"{applescript}\" with administrator privileges")
}

/// Unpacks the `.app.tar.gz` (its top folder dropped) into a temp folder and puts it in the
/// place of `bundle`; the old bundle is moved aside first and back if the new one cannot be
/// moved in. Without write access the move asks for administrator rights.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn replace_bundle(bytes: &[u8], bundle: &Path) -> Result<()> {
    let backup = work_dir("tauri_current_app", bundle)?;
    let extract = work_dir("tauri_updated_app", bundle)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
    let mut files = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        // `Arcalo Mail.app/…` lands in the temp folder; the top folder's own entry sets its mode.
        let inner: PathBuf = entry.path()?.iter().skip(1).collect();
        let dest = extract.path().join(&inner);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&dest)?;
        if !inner.as_os_str().is_empty() {
            files += 1;
        }
    }
    if files == 0 {
        return Err(InstallError::NotInArchive);
    }
    let aside = backup.path().join("current_app");
    match std::fs::rename(bundle, &aside) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return replace_as_admin(bundle, extract.path()),
        Err(e) => return Err(e.into()),
    }
    if let Err(e) = std::fs::rename(extract.path(), bundle) {
        // Never left without an app.
        let _ = std::fs::rename(&aside, bundle);
        return Err(e.into());
    }
    let _ = std::process::Command::new("touch").arg(bundle).status();
    Ok(())
}

/// A temp folder on the volume of `bundle`: a bundle is moved, never copied, and a move does not
/// cross volumes (an app on an external drive). The system's temp folder when it is on that
/// volume, otherwise a hidden folder next to the bundle; when that cannot be made either (no
/// write permission), the system's (the move then goes through the administrator's `mv`).
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn work_dir(prefix: &str, bundle: &Path) -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::Builder::new().prefix(prefix).tempdir()?;
    let Some(parent) = bundle.parent() else { return Ok(tmp) };
    let dev = |p: &Path| p.metadata().map(|m| m.dev()).ok();
    if dev(tmp.path()).is_some() && dev(tmp.path()) == dev(parent) {
        return Ok(tmp);
    }
    Ok(tempfile::Builder::new().prefix(&format!(".{prefix}")).tempdir_in(parent).unwrap_or(tmp))
}

/// The bundle's folder is not writable: `osascript` asks for an administrator's password and
/// moves the new bundle in.
#[cfg(unix)]
fn replace_as_admin(bundle: &Path, new: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("osascript").arg("-e").arg(admin_move_script(bundle, new)).status()?;
        if !status.success() {
            return Err(InstallError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the new app could not be moved into place",
            )));
        }
        let _ = std::process::Command::new("touch").arg(bundle).status();
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bundle, new);
        Err(InstallError::Io(std::io::ErrorKind::PermissionDenied.into()))
    }
}

// ------------------------------------------------------------------ Linux

/// The folders tried for moving the old AppImage aside: the temp folder, the cache folder, the
/// AppImage's own folder (only one on the same file system works).
#[cfg(target_os = "linux")]
fn aside_candidates(appimage: &Path) -> Vec<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
    [Some(std::env::temp_dir()), cache, appimage.parent().map(Path::to_path_buf)].into_iter().flatten().collect()
}

/// Replaces the AppImage in place, keeping its permissions: the old file is moved into a
/// private temp folder on the same file system and moved back if writing the new one fails.
/// The update is the AppImage itself or a `.tar.gz` holding it.
#[cfg(target_os = "linux")]
pub fn replace_appimage(bytes: &[u8], appimage: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dev = appimage.metadata()?.dev();
    for location in aside_candidates(appimage) {
        let Ok(tmp) = tempfile::Builder::new().prefix("tauri_current_app").tempdir_in(&location) else { continue };
        if tmp.path().metadata()?.dev() != dev {
            continue;
        }
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700))?;
        let aside = tmp.path().join("current_app.AppImage");
        let permissions = std::fs::metadata(appimage)?.permissions();
        std::fs::rename(appimage, &aside)?;
        let written = if bytes.starts_with(&[0x1f, 0x8b]) {
            unpack_appimage(bytes, appimage)
        } else {
            std::fs::write(appimage, bytes)
                .and_then(|_| std::fs::set_permissions(appimage, permissions))
                .map_err(InstallError::from)
        };
        if written.is_err() {
            std::fs::rename(&aside, appimage)?;
        }
        return written;
    }
    Err(InstallError::NoTempDir)
}

#[cfg(target_os = "linux")]
fn unpack_appimage(bytes: &[u8], appimage: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
    for mut entry in archive.entries()?.flatten() {
        if entry.path().is_ok_and(|p| p.extension().is_some_and(|x| x == "AppImage")) {
            entry.unpack(appimage)?;
            return Ok(());
        }
    }
    Err(InstallError::NotInArchive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conf() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap()
    }

    fn target(restart: bool, args: &[&str]) -> Target {
        let (mode, installer_args) = windows_config(conf()["plugins"].get("updater"));
        Target {
            app_name: "Arcalo Mail".into(),
            version: "1.0.1".into(),
            exe: PathBuf::new(),
            app_args: args.iter().map(|a| a.to_string()).collect(),
            restart,
            mode,
            installer_args,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-installer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_config_asks_for_the_passive_installer() {
        assert_eq!(windows_config(conf()["plugins"].get("updater")), (InstallMode::Passive, vec![]));
        assert_eq!(windows_config(None).0, InstallMode::Passive);
        let quiet = serde_json::json!({ "windows": { "installMode": "quiet", "installerArgs": ["/D=x"] } });
        assert_eq!(windows_config(Some(&quiet)), (InstallMode::Quiet, vec!["/D=x".to_string()]));
    }

    /// The setup carries the WebView2 bootstrapper instead of downloading it with NSIS' own HTTP
    /// client (which may not get through a proxy), and runs it without asking.
    #[test]
    fn the_setup_carries_the_webview2_bootstrapper() {
        let mode = &conf()["bundle"]["windows"]["webviewInstallMode"];
        assert_eq!(mode["type"], "embedBootstrapper");
        assert_eq!(mode["silent"], true);
    }

    #[test]
    fn nsis_gets_the_plugins_arguments() {
        let none: [String; 0] = [];
        assert_eq!(nsis_parameters(InstallMode::Passive, true, &none, &none), "/P /UPDATE /R /ARGS");
        assert_eq!(nsis_parameters(InstallMode::Passive, false, &none, &none), "/P /UPDATE");
        assert_eq!(nsis_parameters(InstallMode::Quiet, true, &none, &none), "/S /UPDATE /R /ARGS");
        assert_eq!(nsis_parameters(InstallMode::BasicUi, true, &none, &none), "/UPDATE /ARGS");
        let args = ["--profile".to_string(), "C:\\My Mail\\".to_string()];
        assert_eq!(
            nsis_parameters(InstallMode::Passive, true, &args, &["/NS".to_string()]),
            r#"/P /UPDATE /R /ARGS --profile "C:\My Mail\\" /NS"#
        );
        assert_eq!(nsis_parameters(InstallMode::Passive, false, &args, &none), "/P /UPDATE");
    }

    #[test]
    fn nsis_arguments_are_quoted_like_the_plugin_does() {
        assert_eq!(escape_nsis_arg("plain"), "plain");
        assert_eq!(escape_nsis_arg(""), r#""""#);
        assert_eq!(escape_nsis_arg("a b"), r#""a b""#);
        assert_eq!(escape_nsis_arg("/x"), r#""/x""#);
        assert_eq!(escape_nsis_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(escape_nsis_arg(r"C:\dir\"), r"C:\dir\");
        assert_eq!(escape_nsis_arg(r"C:\a b\"), r#""C:\a b\\""#);
    }

    /// The Windows install up to starting the setup ("dry"): the file is written where the setup
    /// runs from and the command line is the plugin's.
    #[test]
    fn a_setup_is_written_and_its_command_built() {
        let root = scratch("nsis");
        let setup = b"MZ\x90\x00 a setup".to_vec();
        let plan = prepare_nsis(&setup, &target(true, &[]), &root).unwrap();
        assert_eq!(plan.file.file_name().unwrap(), "Arcalo Mail-1.0.1-installer.exe");
        let dir = plan.file.parent().unwrap();
        assert_eq!(dir.parent().unwrap(), root);
        assert!(dir.file_name().unwrap().to_str().unwrap().starts_with("Arcalo Mail-1.0.1-updater-"));
        assert_eq!(std::fs::read(&plan.file).unwrap(), setup);
        assert_eq!(plan.parameters, "/P /UPDATE /R /ARGS");
        let quiet = prepare_nsis(&setup, &target(false, &["--x"]), &root).unwrap();
        assert_eq!(quiet.parameters, "/P /UPDATE");
        assert_ne!(quiet.file.parent(), plan.file.parent(), "every install gets its own folder");
        let before = std::fs::read_dir(&root).unwrap().count();
        assert!(matches!(prepare_nsis(b"#!/bin/sh", &target(true, &[]), &root), Err(InstallError::UnknownFormat)));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_bundle_is_found_from_its_binary() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Arcalo Mail.app/Contents/MacOS/arcalo-mail")),
            Some(PathBuf::from("/Applications/Arcalo Mail.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/bin/arcalo-mail")), None);
    }

    #[test]
    fn the_admin_script_quotes_its_paths() {
        assert_eq!(
            admin_move_script(Path::new("/Applications/Arcalo Mail.app"), Path::new("/tmp/new")),
            r#"do shell script "rm -rf '/Applications/Arcalo Mail.app' && mv -f '/tmp/new' '/Applications/Arcalo Mail.app'" with administrator privileges"#
        );
        let odd = admin_move_script(Path::new("/Apps/It's \"A\".app"), Path::new("/tmp/n"));
        assert!(odd.contains(r#"'/Apps/It'\\''s \"A\".app'"#), "{odd}");
    }

    #[cfg(unix)]
    fn tar_gz(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data, mode) in entries {
            let mut header = tar::Header::new_gnu();
            if path.ends_with('/') {
                header.set_entry_type(tar::EntryType::Directory);
            }
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_bundle_is_replaced_with_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("bundle");
        let bundle = root.join("Arcalo Mail.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/arcalo-mail"), "old").unwrap();
        std::fs::write(bundle.join("Contents/old-only"), "gone after the update").unwrap();
        let archive = tar_gz(&[
            ("Arcalo Mail.app/", b"", 0o755),
            ("Arcalo Mail.app/Contents/MacOS/arcalo-mail", b"new", 0o755),
            ("Arcalo Mail.app/Contents/Info.plist", b"<plist/>", 0o644),
        ]);
        replace_bundle(&archive, &bundle).unwrap();
        let exe = bundle.join("Contents/MacOS/arcalo-mail");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(bundle.join("Contents/Info.plist").is_file());
        assert!(!bundle.join("Contents/old-only").exists());
        // A broken archive leaves the bundle as it was.
        assert!(replace_bundle(b"\x1f\x8bnot really", &bundle).is_err());
        assert!(replace_bundle(&tar_gz(&[]), &bundle).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_write_probe_leaves_nothing_behind() {
        let root = scratch("probe");
        assert_eq!(write_probe(&root), None);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(write_probe(&root.join("missing")), Some(std::io::ErrorKind::NotFound));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_is_replaced_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("appimage");
        let appimage = root.join("ArcaloMail_1.0.0_amd64.AppImage");
        std::fs::write(&appimage, "old").unwrap();
        std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o750)).unwrap();
        replace_appimage(b"\x7fELF new", &appimage).unwrap();
        assert_eq!(std::fs::read(&appimage).unwrap(), b"\x7fELF new");
        assert_eq!(std::fs::metadata(&appimage).unwrap().permissions().mode() & 0o777, 0o750);
        let packed = tar_gz(&[("ArcaloMail_1.0.1_amd64.AppImage", b"\x7fELF packed", 0o755)]);
        replace_appimage(&packed, &appimage).unwrap();
        assert_eq!(std::fs::read(&appimage).unwrap(), b"\x7fELF packed");
        // An archive without an AppImage: the old file is back.
        let wrong = tar_gz(&[("readme.txt", b"no", 0o644)]);
        assert!(matches!(replace_appimage(&wrong, &appimage), Err(InstallError::NotInArchive)));
        assert_eq!(std::fs::read(&appimage).unwrap(), b"\x7fELF packed");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "nothing left beside the AppImage");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The updater never gets an address without TLS, and nothing hands the plugin a feed of its
    /// own: the app reads the feed itself (`updates.rs`, through `mail_core::network`).
    #[test]
    fn no_updater_endpoint_is_insecure() {
        let conf = conf();
        let updater = &conf["plugins"]["updater"];
        let endpoints: Vec<&str> = updater["endpoints"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
        assert_eq!(endpoints, [mail_core::update::FEED_URL]);
        let insecure = ["dangerous", "InsecureTransportProtocol"].concat();
        assert!(!include_str!("../tauri.conf.json").contains(&insecure));
        let builder = ["updater", "_builder("].concat();
        let endpoints = [".endpoints", "("].concat();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for needle in [&insecure, &builder, &endpoints] {
                assert!(!text.contains(needle.as_str()), "{} contains {needle}", path.display());
            }
            checked += 1;
        }
        assert!(checked > 5);
    }

    #[test]
    fn errors_are_explained() {
        let full = InstallError::Io(std::io::ErrorKind::StorageFull.into()).message();
        assert!(["Nicht genug freier Speicherplatz", "Not enough free disk space"].contains(&full.as_str()), "{full}");
        let format = InstallError::UnknownFormat.message();
        assert!(format.contains("kein Installationspaket") || format.contains("not an installation package"));
    }
}
