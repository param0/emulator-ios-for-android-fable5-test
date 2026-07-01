//! Scan a directory for `.ipa` files and extract per-app display metadata.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use ios_emu_common::{EmuError, EmuResult};
use zip::ZipArchive;

use crate::plist::{self, PlistValue};

/// Display metadata for one app, sufficient to render a grid tile.
#[derive(Clone, Debug, Default)]
pub struct AppMetadata {
    pub name: String,
    pub bundle_identifier: String,
    pub minimum_os_version: String,
    pub executable: String,
    /// Raw icon bytes (PNG), if one was found in the bundle.
    pub icon_png: Option<Vec<u8>>,
}

/// A discovered IPA on disk, with metadata and the archive-relative paths the
/// launcher needs to unzip and boot it.
#[derive(Clone, Debug)]
pub struct IpaBundle {
    /// Absolute path to the `.ipa` on the host.
    pub ipa_path: PathBuf,
    /// Archive-relative directory of the `.app` (e.g. `Payload/Demo.app`).
    pub bundle_dir: String,
    /// Archive-relative path to the Mach-O executable.
    pub executable_path: String,
    pub metadata: AppMetadata,
}

/// Scan `dir` (non-recursively) for `.ipa` files, returning a bundle descriptor
/// for each one that parses. Files that fail to parse are logged and skipped so
/// one bad archive doesn't hide the rest of the library.
pub fn scan_directory(dir: &Path) -> EmuResult<Vec<IpaBundle>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ipa"))
            != Some(true)
        {
            continue;
        }
        match inspect_ipa(&path) {
            Ok(bundle) => out.push(bundle),
            Err(e) => log::warn!("skipping {}: {e}", path.display()),
        }
    }
    out.sort_by(|a, b| a.metadata.name.to_lowercase().cmp(&b.metadata.name.to_lowercase()));
    Ok(out)
}

/// Open a single IPA and pull out its metadata.
pub fn inspect_ipa(ipa_path: &Path) -> EmuResult<IpaBundle> {
    let file = File::open(ipa_path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| EmuError::Archive(format!("zip open: {e}")))?;

    let bundle_dir = find_bundle_dir(&archive)?;
    let info_path = format!("{bundle_dir}/Info.plist");
    let info_bytes = read_entry(&mut archive, &info_path)?
        .ok_or_else(|| EmuError::Archive(format!("missing {info_path}")))?;

    let plist = plist::parse(&info_bytes).map_err(|e| EmuError::Plist(e.to_string()))?;
    let mut metadata = metadata_from_plist(&plist);

    // Executable name comes from CFBundleExecutable; fall back to the bundle
    // name if absent.
    if metadata.executable.is_empty() {
        metadata.executable = default_executable_name(&bundle_dir);
    }
    let executable_path = format!("{bundle_dir}/{}", metadata.executable);

    // Best-effort icon extraction. Try declared icon files, then conventional
    // names. Missing icons are non-fatal.
    if let Some(icon_name) = pick_icon_name(&plist) {
        for candidate in icon_candidates(&icon_name) {
            let p = format!("{bundle_dir}/{candidate}");
            if let Ok(Some(bytes)) = read_entry(&mut archive, &p) {
                metadata.icon_png = Some(bytes);
                break;
            }
        }
    }

    Ok(IpaBundle { ipa_path: ipa_path.to_path_buf(), bundle_dir, executable_path, metadata })
}

/// Locate the top-level `Payload/<Name>.app` directory inside the archive.
/// Chooses the shallowest match so nested `.app`s (plugins/extensions) don't
/// shadow the main bundle.
fn find_bundle_dir<R: Read + std::io::Seek>(archive: &ZipArchive<R>) -> EmuResult<String> {
    let mut best: Option<String> = None;
    for name in archive.file_names() {
        // Look for ".../Info.plist" whose parent ends in ".app" and sits
        // directly under "Payload/".
        if let Some(dir) = name.strip_suffix("/Info.plist") {
            if dir.ends_with(".app") && dir.starts_with("Payload/") {
                let depth = dir.matches('/').count();
                let take = match &best {
                    None => true,
                    Some(b) => depth < b.matches('/').count(),
                };
                if take {
                    best = Some(dir.to_owned());
                }
            }
        }
    }
    best.ok_or_else(|| EmuError::Archive("no Payload/*.app/Info.plist found".into()))
}

fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> EmuResult<Option<Vec<u8>>> {
    match archive.by_name(name) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf)?;
            Ok(Some(buf))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(EmuError::Archive(format!("read {name}: {e}"))),
    }
}

fn metadata_from_plist(plist: &PlistValue) -> AppMetadata {
    let s = |k: &str| plist.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    // Prefer the user-facing display name, falling back to the bundle name.
    let name = {
        let disp = s("CFBundleDisplayName");
        if disp.is_empty() {
            s("CFBundleName")
        } else {
            disp
        }
    };
    AppMetadata {
        name,
        bundle_identifier: s("CFBundleIdentifier"),
        minimum_os_version: {
            let v = s("MinimumOSVersion");
            if v.is_empty() {
                s("LSMinimumSystemVersion")
            } else {
                v
            }
        },
        executable: s("CFBundleExecutable"),
        icon_png: None,
    }
}

/// Resolve the primary icon filename from the several schemes iOS has used:
///   * `CFBundleIcons` -> `CFBundlePrimaryIcon` -> `CFBundleIconFiles` (iOS 5+),
///   * top-level `CFBundleIconFiles` (iOS 3.2+),
///   * legacy `CFBundleIconFile`.
fn pick_icon_name(plist: &PlistValue) -> Option<String> {
    if let Some(primary) = plist
        .get("CFBundleIcons")
        .and_then(|i| i.get("CFBundlePrimaryIcon"))
        .and_then(|p| p.get("CFBundleIconFiles"))
        .and_then(|f| f.as_array())
        .and_then(|a| a.last())
        .and_then(|v| v.as_str())
    {
        return Some(primary.to_owned());
    }
    if let Some(name) = plist
        .get("CFBundleIconFiles")
        .and_then(|f| f.as_array())
        .and_then(|a| a.last())
        .and_then(|v| v.as_str())
    {
        return Some(name.to_owned());
    }
    plist.get("CFBundleIconFile").and_then(|v| v.as_str()).map(str::to_owned)
}

/// iOS drops the extension and `@2x`/`~ipad` suffixes in the plist; generate the
/// on-disk candidates to probe.
fn icon_candidates(base: &str) -> Vec<String> {
    let stem = base.trim_end_matches(".png");
    vec![
        format!("{stem}@2x.png"),
        format!("{stem}.png"),
        format!("{stem}@3x.png"),
        base.to_owned(),
    ]
}

fn default_executable_name(bundle_dir: &str) -> String {
    bundle_dir
        .rsplit('/')
        .next()
        .and_then(|d| d.strip_suffix(".app"))
        .unwrap_or("App")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_scheme_precedence() {
        use crate::plist;
        let doc = br#"<plist><dict>
            <key>CFBundleIcons</key>
            <dict><key>CFBundlePrimaryIcon</key>
              <dict><key>CFBundleIconFiles</key>
                <array><string>AppIcon60x60</string></array>
              </dict>
            </dict>
        </dict></plist>"#;
        let v = plist::parse(doc).unwrap();
        assert_eq!(pick_icon_name(&v).as_deref(), Some("AppIcon60x60"));
    }

    #[test]
    fn candidates_include_retina() {
        let c = icon_candidates("Icon");
        assert!(c.contains(&"Icon@2x.png".to_string()));
        assert!(c.contains(&"Icon.png".to_string()));
    }
}
