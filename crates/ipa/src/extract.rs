//! Extract an app bundle out of an IPA into a sandbox directory.

use std::io::Read;
use std::path::Path;

use ios_emu_common::{EmuError, EmuResult};
use zip::ZipArchive;

/// Unzip everything under `bundle_dir` (e.g. `Payload/Demo.app`) from `ipa_path`
/// into `dest_root` (the sandbox's `.app` directory), preserving the relative
/// tree. Zip-slip is prevented by rejecting entries whose normalised path would
/// escape `dest_root`.
pub fn extract_app(ipa_path: &Path, bundle_dir: &str, dest_root: &Path) -> EmuResult<()> {
    let file = std::fs::File::open(ipa_path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| EmuError::Archive(format!("zip open: {e}")))?;
    let prefix = format!("{bundle_dir}/");

    for i in 0..archive.len() {
        let mut entry =
            archive.by_index(i).map_err(|e| EmuError::Archive(format!("entry {i}: {e}")))?;
        let name = entry.name().to_owned();
        let Some(rel) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }

        // Reject absolute paths and `..` traversal before touching the disk.
        if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return Err(EmuError::Archive(format!("unsafe archive path: {name}")));
        }

        let out = dest_root.join(rel);
        if name.ends_with('/') {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf)?;
    }
    Ok(())
}
