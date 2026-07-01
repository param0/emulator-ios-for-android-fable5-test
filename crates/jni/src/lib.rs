//! Android integration crate: the JNI bridge plus a pure-Rust orchestration API.
//!
//! Two layers:
//!   * [`session`] — a **pure-Rust** `scan`/`launch` API wiring IPA discovery,
//!     the sandbox, the loader, the core, and the HLE frameworks. No Android
//!     dependency, unit-testable on the host.
//!   * the `extern "system"` entry points below (feature `android`) — thin JNI
//!     marshallers matching `com.iosemu.emulator.emu.EmulatorBridge`. They cross
//!     the boundary with a JSON string (app list), a `byte[]` (icon), and an
//!     `int` (exit code). Built into `libios_emu_jni.so`.

pub mod session;

pub use session::{launch, scan, AppInfo, LaunchOutcome};

// ---------------------------------------------------------------------------
// JNI bridge (device only). Method names follow the JNI mangling for the
// `com.iosemu.emulator.emu.EmulatorBridge` class.
// ---------------------------------------------------------------------------
#[cfg(feature = "android")]
mod bridge {
    #![allow(non_snake_case)]

    use std::path::Path;

    use jni::objects::{JClass, JString};
    use jni::sys::{jbyteArray, jint, jstring};
    use jni::JNIEnv;

    use crate::session;

    /// Install the logcat logger when the library is loaded.
    #[no_mangle]
    pub extern "system" fn JNI_OnLoad(
        _vm: jni::sys::JavaVM,
        _reserved: *mut std::ffi::c_void,
    ) -> jint {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Trace)
                .with_tag("ios-emu"),
        );
        log::info!("ios-emu native library loaded");
        jni::sys::JNI_VERSION_1_6
    }

    /// `String nativeScan(String directory)` -> JSON array of app descriptors.
    #[no_mangle]
    pub extern "system" fn Java_com_iosemu_emulator_emu_EmulatorBridge_nativeScan<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        directory: JString<'local>,
    ) -> jstring {
        let dir = jstring_to_string(&mut env, &directory);
        let apps = session::scan(Path::new(&dir));
        let json = apps_to_json(&apps);
        env.new_string(json).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
    }

    /// `byte[] nativeIcon(String ipaPath)` -> raw PNG bytes, or an empty array.
    #[no_mangle]
    pub extern "system" fn Java_com_iosemu_emulator_emu_EmulatorBridge_nativeIcon<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        ipa_path: JString<'local>,
    ) -> jbyteArray {
        let path = jstring_to_string(&mut env, &ipa_path);
        let bytes = ios_emu_ipa::inspect_ipa(Path::new(&path))
            .ok()
            .and_then(|b| b.metadata.icon_png)
            .unwrap_or_default();
        env.byte_array_from_slice(&bytes)
            .map(|a| a.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    /// `int nativeLaunch(String ipaPath, String containersRoot, String systemRoot)`
    /// -> guest process exit code (negative on host-side failure).
    #[no_mangle]
    pub extern "system" fn Java_com_iosemu_emulator_emu_EmulatorBridge_nativeLaunch<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        ipa_path: JString<'local>,
        containers_root: JString<'local>,
        system_root: JString<'local>,
    ) -> jint {
        let ipa = jstring_to_string(&mut env, &ipa_path);
        let containers = jstring_to_string(&mut env, &containers_root);
        let system = jstring_to_string(&mut env, &system_root);
        let outcome =
            session::launch(Path::new(&ipa), Path::new(&containers), Path::new(&system));
        log::info!("launch outcome: {} ({})", outcome.exit_code, outcome.message);
        outcome.exit_code as jint
    }

    fn jstring_to_string(env: &mut JNIEnv, s: &JString) -> String {
        env.get_string(s).map(|js| js.into()).unwrap_or_default()
    }

    /// Serialise the app list to a compact JSON array by hand (no serde dep).
    fn apps_to_json(apps: &[session::AppInfo]) -> String {
        let mut out = String::from("[");
        for (i, a) in apps.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"bundleId\":\"{}\",\"minOs\":\"{}\",\"ipaPath\":\"{}\",\"hasIcon\":{}}}",
                json_escape(&a.name),
                json_escape(&a.bundle_identifier),
                json_escape(&a.minimum_os_version),
                json_escape(&a.ipa_path),
                a.icon_png.is_some(),
            ));
        }
        out.push(']');
        out
    }

    fn json_escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out
    }
}
