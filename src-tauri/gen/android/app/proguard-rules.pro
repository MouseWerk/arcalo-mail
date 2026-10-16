# Rules of Arcalo Mail for the release build (R8).

# Called from Rust through JNI by name (src-tauri/src/mobile/android.rs): the classes, their
# static methods and the native methods must keep their names.
-keep class de.mousewerk.arcalo.mail.KeystoreCipher { *; }
-keep class de.mousewerk.arcalo.mail.AuthTab { *; }
-keep class de.mousewerk.arcalo.mail.MailNotifier { *; }
-keep class de.mousewerk.arcalo.mail.MailNative { *; }
-keep class de.mousewerk.arcalo.mail.MailCheckWorker { *; }
