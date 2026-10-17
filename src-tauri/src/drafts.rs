//! The open message before the app quits: closing the window, the tray's „Beenden“ and Cmd+Q
//! first ask the UI to save what is being written as a draft, and quit once it answered
//! (`drafts_saved`) or after [`WAIT`]. A UI that does not listen yet (still starting) is not
//! waited for.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// How long a quit waits for the UI at most.
const WAIT: Duration = Duration::from_secs(4);

/// The UI listens for `save-drafts` (it said so with `drafts_listen`).
static LISTENING: AtomicBool = AtomicBool::new(false);
/// Saved for this quit: the window's close already asked, the exit that follows does not again.
static FLUSHED: AtomicBool = AtomicBool::new(false);

fn waiting() -> &'static Mutex<Vec<Sender<()>>> {
    static W: OnceLock<Mutex<Vec<Sender<()>>>> = OnceLock::new();
    W.get_or_init(Mutex::default)
}

/// Whether a quit now asks the UI first (it listens, and this quit did not ask yet).
pub fn needs_save() -> bool {
    LISTENING.load(Ordering::SeqCst) && !FLUSHED.load(Ordering::SeqCst)
}

/// Asks the UI to save the open message, then runs `then` (off the main thread, as it waits).
pub fn save_then(app: &AppHandle, then: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    if !needs_save() {
        then(&app);
        return;
    }
    let (tx, rx) = channel();
    waiting().lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    if app.emit("save-drafts", ()).is_err() {
        FLUSHED.store(true, Ordering::SeqCst);
        then(&app);
        return;
    }
    std::thread::spawn(move || {
        if rx.recv_timeout(WAIT).is_err() {
            mail_core::diag::log("drafts", "no answer from the window before quitting");
        }
        FLUSHED.store(true, Ordering::SeqCst);
        then(&app);
    });
}

/// The UI listens for `save-drafts` from now on.
#[tauri::command]
pub fn drafts_listen() {
    LISTENING.store(true, Ordering::SeqCst);
}

/// The UI saved the open message (or had none).
#[tauri::command]
pub fn drafts_saved() {
    for tx in waiting().lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = tx.send(());
    }
}
