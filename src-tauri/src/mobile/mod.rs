//! What the Android and iOS apps do differently from the desktop. The UI, the store, the
//! commands and the core are the same.
//!
//! - **In front or not.** The account workers (IMAP IDLE, JMAP push) run only while the app is
//!   in front: neither platform lets an app keep a connection open in the background. The UI
//!   says when it is hidden or visible again (`set_in_front`, from `visibilitychange`), Android
//!   also through the activity's pause and resume. In the background the platform runs
//!   `background::run` now and then (Android: WorkManager, iOS: background app refresh).
//! - **Sign-in.** Microsoft's sign-in opens in the platform's browser session (Custom Tabs,
//!   `ASWebAuthenticationSession`) and comes back through the app's own redirect
//!   `de.mousewerk.arcalo.mail://oauth` ([`auth_session`]).
//! - **Badge.** Android: the count on the new-mail notification; iOS: the app icon's number.
//! - **Secrets.** `secrets.rs`: the Android Keystore (through `android`) and the iOS Keychain.
//! - **Notification buttons.** Archive, mark read and reply on a new-mail notification
//!   (Android: the notification's actions, `MailActionReceiver.kt`; iOS: a notification
//!   category). With the app running they go through `crate::notification_action`; Android
//!   without the app acts on the store directly (`background::quick_action`).
//! - **Widget.** Android: `MailWidget.kt` shows `widget.json` (`crate::write_widget`), refreshed
//!   when the unread count changes and after each background check.

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub mod apknotice;
#[cfg(target_os = "ios")]
pub mod ios;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use mail_core::Error;
use tauri::{AppHandle, WindowEvent};

/// Whether the app is in front (its workers run).
static IN_FRONT: AtomicBool = AtomicBool::new(false);
/// The running app, for what the platform hands over (a notification's button).
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The running app; none in Android's background check.
pub fn app() -> Option<&'static AppHandle> {
    APP.get()
}

pub fn in_front() -> bool {
    IN_FRONT.load(Ordering::SeqCst)
}

/// Before Tauri starts: iOS wants its background task registered before the launch ends.
pub fn before_launch() {
    #[cfg(target_os = "ios")]
    ios::register_background();
}

/// In the app's setup, on the main thread.
pub fn setup(app: &AppHandle) {
    let _ = APP.set(app.clone());
    #[cfg(target_os = "android")]
    if let Err(e) = android::init_from_app() {
        mail_core::diag::log("android", &format!("no JNI context: {e}"));
    }
    #[cfg(target_os = "ios")]
    ios::register_categories();
}

/// A notification's button, pressed while the app runs: done off the main thread.
pub fn notification_action(message_id: i64, action: String) {
    let Some(app) = app() else { return };
    std::thread::spawn(move || crate::notification_action(app, message_id, &action));
}

/// Whether the app was started to be used (not by the system for a background check, iOS).
pub fn launched_in_front() -> bool {
    #[cfg(target_os = "ios")]
    let front = !ios::launched_in_background();
    #[cfg(not(target_os = "ios"))]
    let front = true;
    IN_FRONT.store(front, Ordering::SeqCst);
    front
}

/// The app came to the front or went to the background.
pub fn set_in_front(app: &AppHandle, visible: bool) {
    if IN_FRONT.swap(visible, Ordering::SeqCst) == visible {
        return;
    }
    mail_core::diag::log("mobile", if visible { "in front" } else { "in the background" });
    crate::security::on_front(app, visible);
    if visible {
        let app = app.clone();
        // The credential store is asked: not on the main thread.
        std::thread::spawn(move || {
            crate::start_workers(&app);
            crate::update_badge(&app);
        });
    } else {
        crate::stop_workers(app);
        #[cfg(target_os = "ios")]
        ios::schedule_background();
    }
}

/// The activity's pause and resume (Android), coming back from the background (iOS).
pub fn window_event(app: &AppHandle, event: &WindowEvent) {
    match event {
        WindowEvent::Resumed => {
            set_in_front(app, true);
            auth_back_in_app();
        }
        // iOS reports every sheet over the app (the sign-in, the control center) as „resign
        // active“; there the UI's visibility says when the app really went to the background.
        #[cfg(target_os = "android")]
        WindowEvent::Suspended => set_in_front(app, false),
        _ => {}
    }
}

/// The unread count of the inboxes on the badge.
pub fn show_unread(unread: u32) {
    #[cfg(target_os = "android")]
    android::set_unread(unread);
    #[cfg(target_os = "ios")]
    ios::set_badge(unread);
}

/// Asks once whether the app may notify (Android 13 and later, iOS), after an account exists.
pub fn ask_for_notifications(app: &AppHandle) {
    use tauri::plugin::PermissionState;
    use tauri_plugin_notification::NotificationExt;
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.notification().permission_state().unwrap_or(PermissionState::Granted);
        if matches!(state, PermissionState::Prompt | PermissionState::PromptWithRationale) {
            let _ = app.notification().request_permission();
        }
    });
}

// ---------------------------------------------------------------------------- sign-in

type Waiting = tokio::sync::oneshot::Sender<Result<String, Error>>;

/// The sign-in waiting for its redirect, with a number per attempt.
static PENDING: Mutex<Option<(u64, Waiting)>> = Mutex::new(None);
static ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// Opens `url` in the platform's browser session and waits for the redirect to
/// `de.mousewerk.arcalo.mail://oauth` (`mail_core::oauth::sign_in_with_session`).
pub async fn auth_session(url: String) -> Result<String, Error> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let attempt = ATTEMPT.fetch_add(1, Ordering::SeqCst) + 1;
    // An earlier attempt that never came back ends here (its receiver sees the sender go).
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some((attempt, tx));
    #[cfg(target_os = "android")]
    android::open_auth(&url).map_err(Error::Invalid)?;
    #[cfg(target_os = "ios")]
    ios::open_auth(&url);
    rx.await.unwrap_or_else(|_| Err(mail_core::oauth::cancelled()))
}

/// The browser session ended: with the redirect's address, or why not.
pub fn auth_finished(result: Result<String, Error>) {
    if let Some((_, tx)) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(result);
    }
}

/// Android: back in the app. The redirect (`onNewIntent`) arrives before the activity resumes;
/// a sign-in still waiting a moment later was closed in the browser.
fn auth_back_in_app() {
    let Some(attempt) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.0) else { return };
    if cfg!(target_os = "ios") {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if pending.as_ref().is_some_and(|p| p.0 == attempt)
            && let Some((_, tx)) = pending.take()
        {
            let _ = tx.send(Err(mail_core::oauth::cancelled()));
        }
    });
}
