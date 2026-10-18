# Rules of Arcalo Mail for the release build (R8).

# Called from Rust through JNI by name (src-tauri/src/mobile/android.rs): the classes, their
# static methods and the native methods must keep their names.
-keep class de.mousewerk.arcalo.mail.KeystoreCipher { *; }
-keep class de.mousewerk.arcalo.mail.AuthTab { *; }
-keep class de.mousewerk.arcalo.mail.MailNotifier { *; }
-keep class de.mousewerk.arcalo.mail.MailNative { *; }
-keep class de.mousewerk.arcalo.mail.MailCheckWorker { *; }
-keep class de.mousewerk.arcalo.mail.MailWidget { *; }
-keep class de.mousewerk.arcalo.mail.AppFacts { *; }

# Called from the page by name (SystemBars.kt, ArcaloSystemBars in ui/src/lib/viewport.ts).
-keepclassmembers class de.mousewerk.arcalo.mail.SystemBars$Bridge {
    @android.webkit.JavascriptInterface <methods>;
}
