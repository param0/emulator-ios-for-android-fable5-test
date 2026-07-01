//! High-level orchestration: discover apps and launch one end-to-end.

use std::path::{Path, PathBuf};

use ios_emu_core::cpu::NativeExecutor;
use ios_emu_core::{Machine, MachineConfig};
use ios_emu_hle::HleEnvironment;
use ios_emu_ipa::{extract_app, inspect_ipa, scan_directory};
use ios_emu_vfs::Sandbox;

/// One discovered app, flattened for the frontend.
#[derive(Clone, Debug)]
pub struct AppInfo {
    pub name: String,
    pub bundle_identifier: String,
    pub minimum_os_version: String,
    pub ipa_path: String,
    /// PNG icon bytes, if present.
    pub icon_png: Option<Vec<u8>>,
}

/// Result of a launch attempt.
#[derive(Clone, Debug)]
pub struct LaunchOutcome {
    pub exit_code: i32,
    pub message: String,
}

/// Scan `dir` for `.ipa` files and return display metadata for each.
pub fn scan(dir: &Path) -> Vec<AppInfo> {
    match scan_directory(dir) {
        Ok(bundles) => bundles
            .into_iter()
            .map(|b| AppInfo {
                name: b.metadata.name,
                bundle_identifier: b.metadata.bundle_identifier,
                minimum_os_version: b.metadata.minimum_os_version,
                ipa_path: b.ipa_path.to_string_lossy().into_owned(),
                icon_png: b.metadata.icon_png,
            })
            .collect(),
        Err(e) => {
            log::error!("scan {} failed: {e}", dir.display());
            Vec::new()
        }
    }
}

/// Launch the app at `ipa_path`:
///   1. read its metadata,
///   2. prepare an isolated sandbox under `containers_root`,
///   3. unzip the `.app` payload into it,
///   4. map the Mach-O executable and run it under the HLE environment.
///
/// `system_root` is the shared read-only iOS runtime image directory.
pub fn launch(ipa_path: &Path, containers_root: &Path, system_root: &Path) -> LaunchOutcome {
    match launch_inner(ipa_path, containers_root, system_root) {
        Ok(code) => LaunchOutcome { exit_code: code, message: "exited".into() },
        Err(msg) => {
            log::error!("launch failed: {msg}");
            LaunchOutcome { exit_code: -1, message: msg }
        }
    }
}

fn launch_inner(
    ipa_path: &Path,
    containers_root: &Path,
    system_root: &Path,
) -> Result<i32, String> {
    let bundle = inspect_ipa(ipa_path).map_err(|e| e.to_string())?;
    let uuid = generate_uuid();
    log::info!("launching {} ({}) uuid={uuid}", bundle.metadata.name, bundle.metadata.bundle_identifier);

    let sandbox = Sandbox::prepare(containers_root, system_root, &bundle.metadata.name, &uuid)
        .map_err(|e| e.to_string())?;

    // Materialise the app bundle inside the sandbox.
    extract_app(ipa_path, &bundle.bundle_dir, &sandbox.layout().bundle_dir)
        .map_err(|e| e.to_string())?;

    let exe_host: PathBuf = sandbox.layout().bundle_dir.join(&bundle.metadata.executable);
    let bytes = std::fs::read(&exe_host)
        .map_err(|e| format!("read executable {}: {e}", exe_host.display()))?;

    let cfg = MachineConfig::default();
    let mut machine = Machine::load(&cfg, &bytes, sandbox, Box::new(HleEnvironment::new()))
        .map_err(|e| e.to_string())?;
    machine.start_environment();

    // On device this drops into native AArch64 execution; on the host it faults
    // out immediately (there is no CPU backend), which surfaces as an error.
    let mut executor = NativeExecutor::new();
    machine.run(&mut executor).map(|exit| exit.code).map_err(|e| e.to_string())
}

/// Generate a UUID-shaped container id. Not RFC-4122 (that would pull in a crate
/// / a syscall for entropy); it only needs to be collision-resistant enough to
/// name a per-launch directory, for which time + pid + a counter suffice.
fn generate_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;
    let a = nanos ^ (pid << 32);
    let b = nanos.rotate_left(17) ^ seq.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16,
        (b >> 48) as u16,
        b & 0xffff_ffff_ffff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_shape() {
        let u = generate_uuid();
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[4].len(), 12);
        assert_ne!(generate_uuid(), generate_uuid());
    }

    #[test]
    fn scan_empty_dir_is_empty() {
        let dir = std::env::temp_dir().join(format!("ios-emu-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(scan(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
