//! Build script: compile and statically link the native AArch64 backend into
//! the cdylib, but *only* for Android targets. Host builds (used by the unit
//! test-suite) skip this entirely, so `cargo test` needs no C toolchain wiring
//! and the aarch64-only inline assembly is never fed to the host compiler.

use std::path::PathBuf;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Only build the native backend for the real device cdylib: an Android target
    // *and* the `android` feature. This keeps host tests and bare
    // `cargo check --target aarch64-linux-android` free of any C toolchain need.
    let android_feature = std::env::var("CARGO_FEATURE_ANDROID").is_ok();
    if target_os != "android" || !android_feature {
        return;
    }

    // native/ lives two directories up from crates/jni.
    let native = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("native");

    // Recompile whenever any native source or the shared header changes. Emit
    // these *before* `compile()` so an edit to vm_bridge.c always re-triggers the
    // build script and thus the cc invocation.
    for f in ["src/jit_exec.c", "src/vm_bridge.c", "include/ios_emu_jit.h"] {
        println!("cargo:rerun-if-changed={}", native.join(f).display());
    }

    let mut build = cc::Build::new();
    build
        .file(native.join("src/jit_exec.c"))
        .file(native.join("src/vm_bridge.c"))
        .include(native.join("include"))
        .opt_level(2)
        .flag_if_supported("-fno-omit-frame-pointer")
        .warnings(true);
    build.compile("ios_emu_native");

    // `vm_bridge.c` calls __android_log_print; link the NDK log library into the
    // final cdylib.
    println!("cargo:rustc-link-lib=log");
}
