fn main() {
    // The updater's public key is compiled in (`option_env!`); the release workflow sets it.
    println!("cargo:rerun-if-env-changed=ARCALO_MAIL_UPDATER_PUBKEY");
    // Windows: the C runtime is linked statically (.cargo/config.toml, crt-static). Tauri's own
    // switch for that (STATIC_VCRUNTIME, which `cargo tauri build` sets) would link the DLL
    // runtime's import library (ucrt.lib) in again, next to the static one.
    let feature = |f: &str| std::env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|v| v.split(',').any(|x| x == f));
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc") && feature("crt-static") {
        // SAFETY: the build script runs on this one thread; tauri_build reads the variable below.
        unsafe { std::env::set_var("STATIC_VCRUNTIME", "false") };
    }
    tauri_build::build()
}
