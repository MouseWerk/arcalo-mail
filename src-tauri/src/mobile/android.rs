//! Android: the Kotlin side of the app (`gen/android/app/src/main/java/de/mousewerk/arcalo/mail`)
//! through JNI, in both directions.
//!
//! Rust → Kotlin: static methods of `KeystoreCipher` (the secrets, `secrets.rs`), `AuthTab`
//! (Custom Tabs for the sign-in), `MailNotifier` (the unread count on the notification),
//! `MailWidget` (the home-screen widget reads `widget.json` again) and `AppFacts` (version,
//! signing key and installer of the app, for the update notice) and `BiometricGate` (the app
//! lock's fingerprint or face, `security.rs`). They are called from any
//! thread: the classes come from the app's class loader, kept from the first call on a Java
//! thread.
//!
//! Kotlin → Rust: `MailNative.backgroundCheck` (WorkManager's check, in a process that may have
//! no activity and no Tauri), `MailNative.onRedirect` (the sign-in's redirect reached the
//! activity), `MailNative.notificationAction` (archive or mark read from a notification),
//! `MailNative.openMessage` (a notification opened the app to read or answer a message) and
//! `MailNative.openMailto` (an e-mail link of another app).
//!
//! Plain JNI rather than a Tauri plugin: the background check runs without Tauri, and the
//! Keystore must answer there too.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jstring;
use jni::{JNIEnv, JavaVM};

use crate::secrets::Cipher;

const CIPHER: &str = "de.mousewerk.arcalo.mail.KeystoreCipher";
const AUTH_TAB: &str = "de.mousewerk.arcalo.mail.AuthTab";
const NOTIFIER: &str = "de.mousewerk.arcalo.mail.MailNotifier";
const WIDGET: &str = "de.mousewerk.arcalo.mail.MailWidget";
const FACTS: &str = "de.mousewerk.arcalo.mail.AppFacts";
const BIOMETRIC: &str = "de.mousewerk.arcalo.mail.BiometricGate";

/// WorkManager gives a worker ten minutes; the check takes at most this.
const BACKGROUND_BUDGET: Duration = Duration::from_secs(90);

struct Java {
    vm: JavaVM,
    /// The app's class loader (a thread Rust started only sees the system's).
    loader: GlobalRef,
    /// The application context.
    context: GlobalRef,
}

static JAVA: OnceLock<Java> = OnceLock::new();
/// The running app's activity (Custom Tabs start from it); none in the background check.
static ACTIVITY: Mutex<Option<GlobalRef>> = Mutex::new(None);

/// The message of a Java exception (cleared), else the JNI error.
fn failure(env: &mut JNIEnv, e: jni::errors::Error) -> String {
    if env.exception_check().unwrap_or(false) {
        let thrown = env.exception_occurred();
        let _ = env.exception_clear();
        if let Ok(t) = thrown
            && let Ok(text) = env.call_method(&t, "toString", "()Ljava/lang/String;", &[]).and_then(|v| v.l())
            && let Ok(text) = env.get_string(&JString::from(text))
        {
            return text.into();
        }
    }
    e.to_string()
}

/// Keeps the JVM, the application context and its class loader (the first call wins).
fn init_with(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    if JAVA.get().is_some() {
        return Ok(());
    }
    let app = env
        .call_method(context, "getApplicationContext", "()Landroid/content/Context;", &[])
        .and_then(|v| v.l())
        .map_err(|e| failure(env, e))?;
    let loader = env
        .call_method(&app, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(|e| failure(env, e))?;
    let java = Java {
        vm: env.get_java_vm().map_err(|e| e.to_string())?,
        loader: env.new_global_ref(loader).map_err(|e| e.to_string())?,
        context: env.new_global_ref(app).map_err(|e| e.to_string())?,
    };
    let _ = JAVA.set(java);
    Ok(())
}

/// The running app: the JVM and the activity that Tauri's runtime registered.
pub fn init_from_app() -> Result<(), String> {
    let ctx = ndk_context::android_context();
    // SAFETY: ndk-context holds the JVM and a global reference to the activity for the life of
    // the process; neither is released here.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    init_with(&mut env, &activity)?;
    *ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(env.new_global_ref(&activity).map_err(|e| e.to_string())?);
    Ok(())
}

/// Runs `f` with a JNI environment on this thread, in a local frame of its own.
fn with_env<R>(f: impl FnOnce(&mut JNIEnv, &Java) -> Result<R, String>) -> Result<R, String> {
    let java = JAVA.get().ok_or_else(|| "the Android side is not ready".to_string())?;
    let mut env = java.vm.attach_current_thread().map_err(|e| e.to_string())?;
    env.push_local_frame(32).map_err(|e| e.to_string())?;
    let r = f(&mut env, java);
    // SAFETY: nothing of the frame is used after it is popped.
    let _ = unsafe { env.pop_local_frame(&JObject::null()) };
    r
}

/// A class of the app.
fn class<'l>(env: &mut JNIEnv<'l>, java: &Java, name: &str) -> Result<JClass<'l>, String> {
    let n = env.new_string(name).map_err(|e| e.to_string())?;
    let c = env
        .call_method(java.loader.as_obj(), "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&n)])
        .and_then(|v| v.l())
        .map_err(|e| failure(env, e))?;
    Ok(JClass::from(c))
}

/// `String Class.method(String)`.
fn string_call(class_name: &str, method: &str, arg: &str) -> Result<String, String> {
    with_env(|env, java| {
        let c = class(env, java, class_name)?;
        let a = env.new_string(arg).map_err(|e| e.to_string())?;
        let out = env
            .call_static_method(&c, method, "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&a)])
            .and_then(|v| v.l())
            .map_err(|e| failure(env, e))?;
        let text: String = env.get_string(&JString::from(out)).map_err(|e| e.to_string())?.into();
        Ok(text)
    })
}

/// AES-GCM with the app's key in the Android Keystore (`KeystoreCipher.kt`).
pub struct KeystoreCipher;

impl Cipher for KeystoreCipher {
    fn seal(&self, plain: &str) -> Result<String, String> {
        string_call(CIPHER, "seal", plain)
    }

    fn open(&self, sealed: &str) -> Result<String, String> {
        string_call(CIPHER, "open", sealed)
    }
}

/// Opens the sign-in in a Custom Tab over the app (`AuthTab.kt`).
pub fn open_auth(url: &str) -> Result<(), String> {
    let activity = ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("no activity")?;
    with_env(|env, java| {
        let c = class(env, java, AUTH_TAB)?;
        let u = env.new_string(url).map_err(|e| e.to_string())?;
        env.call_static_method(
            &c,
            "open",
            "(Landroid/app/Activity;Ljava/lang/String;)V",
            &[JValue::Object(activity.as_obj()), JValue::Object(&u)],
        )
        .map_err(|e| failure(env, e))?;
        Ok(())
    })
}

/// Whether the phone can confirm the user with a fingerprint, the face or the screen lock
/// (`BiometricGate.kt`, Android 9 and later).
pub fn biometric_available() -> bool {
    let activity = ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(activity) = activity else { return false };
    with_env(|env, java| {
        let c = class(env, java, BIOMETRIC)?;
        env.call_static_method(&c, "available", "(Landroid/app/Activity;)Z", &[JValue::Object(activity.as_obj())])
            .and_then(|v| v.z())
            .map_err(|e| failure(env, e))
    })
    .unwrap_or(false)
}

/// Asks for the fingerprint, the face or the screen lock (`BiometricGate.kt`); waits for the
/// answer, so never on the main thread.
pub fn biometric_verify(title: &str) -> bool {
    let activity = ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(activity) = activity else { return false };
    let r = with_env(|env, java| {
        let c = class(env, java, BIOMETRIC)?;
        let t = env.new_string(title).map_err(|e| e.to_string())?;
        env.call_static_method(
            &c,
            "verify",
            "(Landroid/app/Activity;Ljava/lang/String;)Z",
            &[JValue::Object(activity.as_obj()), JValue::Object(&t)],
        )
        .and_then(|v| v.z())
        .map_err(|e| failure(env, e))
    });
    r.unwrap_or_else(|e| {
        mail_core::diag::log("android", &format!("biometric: {e}"));
        false
    })
}

/// The unread count on the new-mail notification; none left: the notifications go.
pub fn set_unread(unread: u32) {
    let r = with_env(|env, java| {
        let c = class(env, java, NOTIFIER)?;
        env.call_static_method(
            &c,
            "setUnread",
            "(Landroid/content/Context;I)V",
            &[JValue::Object(java.context.as_obj()), JValue::Int(unread.min(i32::MAX as u32) as i32)],
        )
        .map_err(|e| failure(env, e))?;
        Ok(())
    });
    if let Err(e) = r {
        mail_core::diag::log("android", &format!("badge: {e}"));
    }
}

/// The home-screen widget reads `widget.json` again (`MailWidget.kt`).
pub fn refresh_widget() {
    let r = with_env(|env, java| {
        let c = class(env, java, WIDGET)?;
        env.call_static_method(&c, "refresh", "(Landroid/content/Context;)V", &[JValue::Object(java.context.as_obj())])
            .map_err(|e| failure(env, e))?;
        Ok(())
    });
    if let Err(e) = r {
        mail_core::diag::log("android", &format!("widget: {e}"));
    }
}

/// The installed app as Android's package manager reads it (`AppFacts.kt`): version, signing
/// certificate and the package that installed it.
pub fn installed() -> Result<mail_core::update_android::Installed, String> {
    let json = with_env(|env, java| {
        let c = class(env, java, FACTS)?;
        let out = env
            .call_static_method(
                &c,
                "facts",
                "(Landroid/content/Context;)Ljava/lang/String;",
                &[JValue::Object(java.context.as_obj())],
            )
            .and_then(|v| v.l())
            .map_err(|e| failure(env, e))?;
        let text: String = env.get_string(&JString::from(out)).map_err(|e| e.to_string())?.into();
        Ok(text)
    })?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// `MailNative.notificationAction(context, dataDir, id, action)`: archive (`archive`) or mark
/// read (`read`) from a notification's button. With the app running through its state (the
/// worker carries it to the server at once); else in the store, for the next check. Answers the
/// unread count, or -1 when the app does it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_notificationAction<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    context: JObject<'l>,
    data_dir: JString<'l>,
    id: jni::sys::jlong,
    action: JString<'l>,
) -> jni::sys::jint {
    let action: String = env.get_string(&action).map(Into::into).unwrap_or_default();
    if super::app().is_some() {
        super::notification_action(id, action);
        return -1;
    }
    let dir: String = env.get_string(&data_dir).map(Into::into).unwrap_or_default();
    if let Err(e) = init_with(&mut env, &context) {
        mail_core::diag::log("android", &format!("notification action without JNI: {e}"));
    }
    match crate::background::quick_action(Path::new(&dir), id, &action) {
        Some(unread) => {
            refresh_widget();
            unread.min(i32::MAX as u32) as i32
        }
        None => -1,
    }
}

/// `MailNative.openMessage(id, reply)`: a notification opened the app on a message.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_openMessage<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    id: jni::sys::jlong,
    reply: jni::sys::jboolean,
) {
    crate::commands::open_message(super::app(), id, reply != 0);
}

/// `MailNative.openMailto(url)`: an e-mail link of another app opens the compose sheet.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_openMailto<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    url: JString<'l>,
) {
    let url: String = env.get_string(&url).map(Into::into).unwrap_or_default();
    if let Some(m) = mail_core::mailto::parse(&url) {
        crate::defaultmail::open(super::app(), m);
    }
}

/// `MailNative.backgroundCheck(context, dataDir)`: one check of every account (WorkManager);
/// answers the `background::Report` as JSON. Skipped while the app is in front.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_backgroundCheck<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    context: JObject<'l>,
    data_dir: JString<'l>,
) -> jstring {
    let dir: String = env.get_string(&data_dir).map(Into::into).unwrap_or_default();
    let report = if super::in_front() {
        crate::background::Report { skipped: true, ..Default::default() }
    } else {
        match init_with(&mut env, &context) {
            Ok(()) => crate::background::run(Path::new(&dir), BACKGROUND_BUDGET),
            Err(e) => {
                mail_core::diag::log("android", &format!("background check without JNI: {e}"));
                crate::background::Report { skipped: true, ..Default::default() }
            }
        }
    };
    let json = serde_json::to_string(&report).unwrap_or_else(|_| "{}".into());
    env.new_string(json).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// `MailNative.onRedirect(url)`: the sign-in came back to the app.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_onRedirect<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    url: JString<'l>,
) {
    let url: String = env.get_string(&url).map(Into::into).unwrap_or_default();
    super::auth_finished(Ok(url));
}

/// `MailNative.cancelCheck()`: WorkManager stops the worker.
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_mousewerk_arcalo_mail_MailNative_cancelCheck<'l>(_env: JNIEnv<'l>, _class: JClass<'l>) {
    crate::background::cancel();
}
