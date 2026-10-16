//! iOS: UIKit, AuthenticationServices, BackgroundTasks and UserNotifications through the
//! Objective-C runtime, so the generated Xcode project needs no code of its own (the frameworks
//! are linked through `bundle.iOS.frameworks`, the background mode and the task's identifier are
//! in `Info.ios.plist`).
//!
//! - The sign-in: `ASWebAuthenticationSession` with the callback scheme of
//!   `de.mousewerk.arcalo.mail://oauth`; it shares Safari's cookies, so a signed-in Microsoft
//!   account needs no password again.
//! - The background check: a `BGAppRefreshTask`, registered before the launch ends and asked
//!   for again each time the app goes to the background (at the earliest after 15 minutes; the
//!   system decides when, often hours later). It runs `background::run` for 25 seconds and posts
//!   local notifications.
//! - The badge: `applicationIconBadgeNumber` (the notification permission includes it).

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::time::Duration;

use block2::{Block, RcBlock};
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{ClassType, define_class, msg_send};

use crate::background::{self, Report};

/// The background task's identifier (also in `Info.ios.plist`, BGTaskSchedulerPermittedIdentifiers).
const TASK: &str = "de.mousewerk.arcalo.mail.refresh";
/// The scheme of `mail_core::oauth::MOBILE_REDIRECT`.
const CALLBACK_SCHEME: &str = "de.mousewerk.arcalo.mail";
/// Background app refresh gives about 30 seconds.
const BACKGROUND_BUDGET: Duration = Duration::from_secs(25);
/// The earliest next check after the app went to the background.
const NEXT_CHECK_SECS: f64 = 15.0 * 60.0;
/// `UIApplicationStateBackground`.
const STATE_BACKGROUND: isize = 2;
/// `ASWebAuthenticationSessionErrorCodeCanceledLogin`.
const CANCELED_LOGIN: isize = 1;

fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

/// An `NSString` (autoreleased, retained here).
fn ns_string(s: &str) -> Retained<AnyObject> {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    // SAFETY: a valid C string; the result is retained.
    unsafe { msg_send![class(c"NSString").expect("Foundation"), stringWithUTF8String: c.as_ptr()] }
}

/// The text of an `NSString`.
fn rust_string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: `s` is an NSString; UTF8String lives as long as the string.
    let p: *const std::ffi::c_char = unsafe { msg_send![s, UTF8String] };
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn shared_application() -> *mut AnyObject {
    match class(c"UIApplication") {
        // SAFETY: the shared application exists once UIKit runs.
        Some(c) => unsafe { msg_send![c, sharedApplication] },
        None => std::ptr::null_mut(),
    }
}

/// The app's data folder, as Tauri's `app_data_dir` on iOS.
fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support").join("de.mousewerk.arcalo.mail")
}

/// Whether the system launched the app in the background (for its check): then no worker
/// starts. On the main thread, in the setup.
pub fn launched_in_background() -> bool {
    let app = shared_application();
    if app.is_null() {
        return false;
    }
    // SAFETY: on the main thread (Tauri's setup).
    let state: isize = unsafe { msg_send![app, applicationState] };
    state == STATE_BACKGROUND
}

/// The number on the app icon.
pub fn set_badge(unread: u32) {
    DispatchQueue::main().exec_async(move || {
        let app = shared_application();
        if !app.is_null() {
            // SAFETY: on the main thread.
            let _: () = unsafe { msg_send![app, setApplicationIconBadgeNumber: unread as isize] };
        }
    });
}

// ---------------------------------------------------------------------------- sign-in

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[name = "ArcaloMailAuthAnchor"]
    /// The window `ASWebAuthenticationSession` shows its sheet over.
    struct AuthAnchor;

    impl AuthAnchor {
        #[unsafe(method(presentationAnchorForWebAuthenticationSession:))]
        fn anchor(&self, _session: *mut AnyObject) -> *mut AnyObject {
            key_window()
        }
    }
);

fn key_window() -> *mut AnyObject {
    let app = shared_application();
    if app.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: called by the session on the main thread.
    unsafe {
        let key: *mut AnyObject = msg_send![app, keyWindow];
        if !key.is_null() {
            return key;
        }
        let windows: *mut AnyObject = msg_send![app, windows];
        if windows.is_null() { std::ptr::null_mut() } else { msg_send![windows, firstObject] }
    }
}

thread_local! {
    /// The running session and its anchor, kept until it ends (main thread only).
    static SESSION: RefCell<Option<(Retained<AnyObject>, Retained<AnyObject>)>> = const { RefCell::new(None) };
}

/// Opens `url` in an `ASWebAuthenticationSession`; the redirect or the cancel goes to
/// `mobile::auth_finished`.
pub fn open_auth(url: &str) {
    let url = url.to_string();
    DispatchQueue::main().exec_async(move || {
        autoreleasepool(|_| {
            if let Err(e) = start_session(&url) {
                super::auth_finished(Err(mail_core::Error::Invalid(e)));
            }
        })
    });
}

fn start_session(url: &str) -> Result<(), String> {
    let session_class = class(c"ASWebAuthenticationSession").ok_or("AuthenticationServices is missing")?;
    let ns_url_class = class(c"NSURL").ok_or("Foundation is missing")?;
    // SAFETY: the documented initializers and setters of ASWebAuthenticationSession (iOS 12+),
    // on the main thread; the block is copied by the session.
    unsafe {
        let ns_url: *mut AnyObject = msg_send![ns_url_class, URLWithString: &*ns_string(url)];
        if ns_url.is_null() {
            return Err("invalid sign-in address".into());
        }
        let done = RcBlock::new(|callback: *mut AnyObject, error: *mut AnyObject| {
            let result = if !callback.is_null() {
                let text: *mut AnyObject = msg_send![callback, absoluteString];
                rust_string(text).ok_or_else(mail_core::oauth::cancelled)
            } else {
                let code: isize = if error.is_null() { CANCELED_LOGIN } else { msg_send![error, code] };
                if code == CANCELED_LOGIN {
                    Err(mail_core::oauth::cancelled())
                } else {
                    let text: *mut AnyObject = msg_send![error, localizedDescription];
                    Err(mail_core::Error::Invalid(rust_string(text).unwrap_or_default()))
                }
            };
            super::auth_finished(result);
            SESSION.with(|s| s.borrow_mut().take());
        });
        let allocated: *mut AnyObject = msg_send![session_class, alloc];
        let session: *mut AnyObject = msg_send![
            allocated,
            initWithURL: ns_url,
            callbackURLScheme: &*ns_string(CALLBACK_SCHEME),
            completionHandler: &*done
        ];
        let session = Retained::from_raw(session).ok_or("no session")?;
        let anchor: Retained<AnyObject> = msg_send![AuthAnchor::class(), new];
        let _: () = msg_send![&*session, setPresentationContextProvider: &*anchor];
        let _: () = msg_send![&*session, setPrefersEphemeralWebBrowserSession: false];
        let started: bool = msg_send![&*session, start];
        if !started {
            return Err("the sign-in could not be shown".into());
        }
        SESSION.with(|s| *s.borrow_mut() = Some((session, anchor)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------- background

/// Registers the background task; before the launch ends (`run`, before Tauri starts).
pub fn register_background() {
    let Some(scheduler_class) = class(c"BGTaskScheduler") else { return };
    autoreleasepool(|_| {
        let handler = RcBlock::new(|task: *mut AnyObject| run_task(task));
        // SAFETY: the documented registration of BGTaskScheduler (iOS 13+); a nil queue runs the
        // handler on a background queue.
        unsafe {
            let scheduler: *mut AnyObject = msg_send![scheduler_class, sharedScheduler];
            let ok: bool = msg_send![
                scheduler,
                registerForTaskWithIdentifier: &*ns_string(TASK),
                usingQueue: std::ptr::null_mut::<AnyObject>(),
                launchHandler: &*handler
            ];
            if !ok {
                mail_core::diag::log("ios", "background task not registered (Info.plist)");
            }
        }
    });
}

/// Asks for the next background check (the app went to the background).
pub fn schedule_background() {
    let (Some(scheduler_class), Some(request_class), Some(date_class)) =
        (class(c"BGTaskScheduler"), class(c"BGAppRefreshTaskRequest"), class(c"NSDate"))
    else {
        return;
    };
    autoreleasepool(|_| {
        // SAFETY: the documented request API (iOS 13+); a refused request (Background App
        // Refresh switched off) only means no check.
        unsafe {
            let allocated: *mut AnyObject = msg_send![request_class, alloc];
            let request: *mut AnyObject = msg_send![allocated, initWithIdentifier: &*ns_string(TASK)];
            let Some(request) = Retained::from_raw(request) else { return };
            let when: *mut AnyObject = msg_send![date_class, dateWithTimeIntervalSinceNow: NEXT_CHECK_SECS];
            let _: () = msg_send![&*request, setEarliestBeginDate: when];
            let scheduler: *mut AnyObject = msg_send![scheduler_class, sharedScheduler];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let ok: bool = msg_send![scheduler, submitTaskRequest: &*request, error: &mut error];
            if !ok {
                mail_core::diag::log("ios", "background check not scheduled");
            }
        }
    });
}

/// The system runs the background task: one check, the notifications, the badge, the next one.
fn run_task(task: *mut AnyObject) {
    // SAFETY: `task` is the BGTask the system hands to the launch handler.
    let Some(task) = (unsafe { Retained::retain(task) }) else { return };
    let expire = RcBlock::new(background::cancel);
    // SAFETY: setExpirationHandler: copies the block.
    let _: () = unsafe { msg_send![&*task, setExpirationHandler: &*expire] };
    schedule_background();
    let report = if super::in_front() {
        Report { skipped: true, ..Default::default() }
    } else {
        background::run(&data_dir(), BACKGROUND_BUDGET)
    };
    autoreleasepool(|_| post(&report));
    if !report.skipped {
        set_badge(report.unread);
    }
    // SAFETY: completing the task the system handed over.
    let _: () = unsafe { msg_send![&*task, setTaskCompletedWithSuccess: true] };
}

/// Local notifications for what the check found.
fn post(report: &Report) {
    let (Some(center_class), Some(content_class), Some(request_class)) =
        (class(c"UNUserNotificationCenter"), class(c"UNMutableNotificationContent"), class(c"UNNotificationRequest"))
    else {
        return;
    };
    // SAFETY: the documented UserNotifications API; it may be used from any thread.
    unsafe {
        let center: *mut AnyObject = msg_send![center_class, currentNotificationCenter];
        for note in &report.notes {
            let content: Retained<AnyObject> = msg_send![content_class, new];
            let _: () = msg_send![&*content, setTitle: &*ns_string(&note.title)];
            let _: () = msg_send![&*content, setBody: &*ns_string(&note.body)];
            let _: () = msg_send![&*content, setThreadIdentifier: &*ns_string(&note.group)];
            if let Some(number_class) = class(c"NSNumber") {
                let badge: *mut AnyObject = msg_send![number_class, numberWithInteger: report.unread as isize];
                let _: () = msg_send![&*content, setBadge: badge];
            }
            if let Some(sound_class) = class(c"UNNotificationSound") {
                let sound: *mut AnyObject = msg_send![sound_class, defaultSound];
                let _: () = msg_send![&*content, setSound: sound];
            }
            let request: *mut AnyObject = msg_send![
                request_class,
                requestWithIdentifier: &*ns_string(&note.key),
                content: &*content,
                trigger: std::ptr::null_mut::<AnyObject>()
            ];
            let no_handler: Option<&Block<dyn Fn(*mut AnyObject)>> = None;
            let _: () = msg_send![center, addNotificationRequest: request, withCompletionHandler: no_handler];
        }
    }
}
