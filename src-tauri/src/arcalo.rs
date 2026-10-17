//! „An Arcalo senden“: the mail as a page or a task in Arcalo through its command line (`arcalo
//! pages create … --stdin`, `arcalo tasks add …`) when `arcalo` is on the PATH; else the UI saves
//! the page as a Markdown file. What is handed over is built in the core (`mail_core::arcalo`);
//! nothing goes over the network.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mail_core::Error;
use mail_core::arcalo::{self, Export};
use mail_core::settings::ArcaloTarget;
use serde::Serialize;
use tauri::State;

use crate::AppState;

type Res<T> = Result<T, Error>;

/// Longer than this, the command is stopped (its own process only).
const TIMEOUT: Duration = Duration::from_secs(20);

fn cli() -> Option<std::path::PathBuf> {
    arcalo::find_cli(std::env::var_os("PATH").as_deref())
}

/// Whether Arcalo's command line is there (its path).
#[tauri::command]
pub fn arcalo_status() -> Option<String> {
    cli().map(|p| p.to_string_lossy().into_owned())
}

#[derive(Serialize)]
pub struct Sent {
    /// Arcalo took it; `title`: the page's path or the task's text as Arcalo says it.
    via_cli: bool,
    title: String,
    /// Without Arcalo: the page to save as a file.
    markdown: Option<String>,
    file_name: Option<String>,
}

/// Runs `arcalo` with `args` (and `input` on stdin); its JSON answer.
fn run(program: &std::path::Path, args: &[&str], input: Option<&str>) -> Res<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for the call.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| {
        Error::Invalid(mail_core::trf!("Arcalo lässt sich nicht starten: {e}", "Arcalo cannot be started: {e}"))
    })?;
    // The page on stdin; closed afterwards (dropped), also without input.
    if let Some(mut stdin) = child.stdin.take()
        && let Some(text) = input
    {
        let _ = stdin.write_all(text.as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Invalid(
                    mail_core::tr!("Arcalo antwortet nicht.", "Arcalo does not answer.").into(),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(Error::Invalid(e.to_string())),
        }
    }
    let out = child.wait_with_output().map_err(|e| Error::Invalid(e.to_string()))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::Invalid(mail_core::trf!("Arcalo meldet: {why}", "Arcalo reports: {why}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn as_strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// The mail as a page or a task in Arcalo; without Arcalo the page for a Markdown file.
#[tauri::command]
pub async fn send_to_arcalo(state: State<'_, AppState>, message_id: i64, target: Option<ArcaloTarget>) -> Res<Sent> {
    let (export, target): (Export, ArcaloTarget) = {
        let store = state.store();
        let target = match target {
            Some(t) => t,
            None => mail_core::settings::load(&store)?.arcalo_target,
        };
        let view = mail_core::actions::message_view(&store, message_id, mail_core::settings::RemoteContent::Ask)?;
        let text = match view.detail.text.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(t) => t.to_string(),
            None => mail_core::compose::html_to_text(view.body.full_html.as_deref().unwrap_or(&view.body.html)),
        };
        (arcalo::export(&view.detail, &text), target)
    };
    let Some(program) = cli() else {
        return Ok(Sent {
            via_cli: false,
            title: export.title,
            markdown: Some(export.markdown),
            file_name: Some(export.file_name),
        });
    };
    let answer = tauri::async_runtime::spawn_blocking(move || {
        let out = match target {
            ArcaloTarget::Page => run(&program, &as_strs(&arcalo::page_args(&export.title)), Some(&export.markdown)),
            ArcaloTarget::Task => run(&program, &as_strs(&arcalo::task_args(&export.task)), None),
        }?;
        // Done only when Arcalo says what it made.
        arcalo::answer(&out).ok_or_else(|| {
            Error::Invalid(mail_core::tr!("Arcalo hat nichts angelegt.", "Arcalo did not create anything.").into())
        })
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))??;
    Ok(Sent { via_cli: true, title: answer, markdown: None, file_name: None })
}

/// Saves the Markdown of a page where the user chose.
#[tauri::command]
pub fn save_markdown(path: String, text: String) -> Res<()> {
    std::fs::write(&path, text).map_err(|e| Error::Invalid(e.to_string()))
}
