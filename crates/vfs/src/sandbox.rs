//! Sandbox container creation and iOS->host path translation.

use std::path::{Path, PathBuf};

use ios_emu_common::{EmuError, EmuResult};

use crate::translate::{normalize_ios_path, TranslationError};

/// The canonical iOS directory names created inside every container.
#[derive(Clone, Debug)]
pub struct SandboxLayout {
    pub bundle_dir: PathBuf,   // <root>/<AppName>.app
    pub documents: PathBuf,    // <root>/Documents
    pub library: PathBuf,      // <root>/Library
    pub caches: PathBuf,       // <root>/Library/Caches
    pub preferences: PathBuf,  // <root>/Library/Preferences
    pub tmp: PathBuf,          // <root>/tmp
}

/// An isolated per-app filesystem rooted at a host directory.
pub struct Sandbox {
    /// Host directory that backs the whole container. Nothing the guest does
    /// may resolve outside this directory.
    host_root: PathBuf,
    /// The iOS-absolute prefix the container is mounted at, e.g.
    /// `/var/mobile/Applications/<UUID>`. Guest paths under this prefix map into
    /// `host_root`; well-known read-only trees (`/System`, `/usr`) map into a
    /// shared runtime image.
    ios_prefix: String,
    /// UUID assigned to this application container.
    uuid: String,
    layout: SandboxLayout,
    /// Host directory holding the emulated read-only system image
    /// (`/System`, `/usr/lib`, ...). Shared across apps.
    system_root: PathBuf,
}

impl Sandbox {
    /// Prepare (creating on disk) a sandbox for `app_name` under
    /// `containers_root/Applications/<uuid>`. `system_root` is the shared,
    /// read-only iOS runtime image directory.
    pub fn prepare(
        containers_root: &Path,
        system_root: &Path,
        app_name: &str,
        uuid: &str,
    ) -> EmuResult<Sandbox> {
        let host_root = containers_root.join("Applications").join(uuid);
        let bundle_name = format!("{}.app", sanitize_component(app_name));
        let layout = SandboxLayout {
            bundle_dir: host_root.join(&bundle_name),
            documents: host_root.join("Documents"),
            library: host_root.join("Library"),
            caches: host_root.join("Library").join("Caches"),
            preferences: host_root.join("Library").join("Preferences"),
            tmp: host_root.join("tmp"),
        };

        for dir in [
            &layout.bundle_dir,
            &layout.documents,
            &layout.caches,
            &layout.preferences,
            &layout.tmp,
        ] {
            std::fs::create_dir_all(dir)?;
        }

        Ok(Sandbox {
            ios_prefix: format!("/var/mobile/Applications/{uuid}"),
            uuid: uuid.to_owned(),
            host_root,
            layout,
            system_root: system_root.to_path_buf(),
        })
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn layout(&self) -> &SandboxLayout {
        &self.layout
    }

    pub fn ios_prefix(&self) -> &str {
        &self.ios_prefix
    }

    /// The guest-visible absolute path of the app bundle directory.
    pub fn bundle_ios_path(&self) -> String {
        let name = self
            .layout
            .bundle_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("App.app");
        format!("{}/{}", self.ios_prefix, name)
    }

    /// Translate a guest-absolute iOS path to a concrete host path, enforcing
    /// containment. Returns `(host_path, read_only)`.
    ///
    /// Routing rules:
    ///   * paths under the container prefix -> `host_root`,
    ///   * `/System`, `/usr`, `/Library` (system), `/private/var/db` -> the
    ///     shared read-only system image,
    ///   * everything else is rejected (the guest has no business there).
    pub fn translate(&self, ios_path: &str) -> EmuResult<(PathBuf, bool)> {
        let path = ios_path.trim();
        // Relative paths are resolved against the app bundle (iOS apps run with
        // cwd == bundle for many APIs); make them absolute first.
        let absolute = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("{}/{}", self.bundle_ios_path(), path)
        };

        if let Some(rest) = strip_prefix_dir(&absolute, &self.ios_prefix) {
            let host = self.join_checked(&self.host_root, rest)?;
            return Ok((host, false));
        }
        for ro in ["/System", "/usr", "/Library", "/private/var/db", "/Applications"] {
            if let Some(rest) = strip_prefix_dir(&absolute, ro) {
                let base = self.system_root.join(ro.trim_start_matches('/'));
                let host = self.join_checked(&base, rest)?;
                return Ok((host, true));
            }
        }
        Err(EmuError::Vfs(format!("path outside sandbox: {ios_path}")))
    }

    /// Normalise `rest` and join it under `base`, guaranteeing the result stays
    /// within `base`.
    fn join_checked(&self, base: &Path, rest: &str) -> EmuResult<PathBuf> {
        let comps = normalize_ios_path(rest).map_err(|e| match e {
            TranslationError::Escape => EmuError::Vfs(format!("sandbox escape: {rest}")),
            TranslationError::Malformed => EmuError::Vfs(format!("bad path: {rest}")),
        })?;
        let mut host = base.to_path_buf();
        for c in comps {
            host.push(c);
        }
        Ok(host)
    }
}

/// If `path` is exactly `prefix` or lives under `prefix/`, return the trailing
/// remainder (possibly empty). Component-aware so `/usrlocal` does not match
/// prefix `/usr`.
fn strip_prefix_dir<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        Some("")
    } else if let Some(sub) = rest.strip_prefix('/') {
        Some(sub)
    } else {
        None
    }
}

/// Strip path separators from a user-controlled bundle name so it can't create
/// the `.app` directory outside the container.
fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "App".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ios-emu-vfs-test-{}", std::process::id()));
        p
    }

    #[test]
    fn prepare_and_translate() {
        let root = temp_dir();
        let sys = root.join("system");
        let sb = Sandbox::prepare(&root, &sys, "My App", "ABCD-UUID").unwrap();

        // Container paths map read-write into host_root.
        let (host, ro) = sb.translate("/var/mobile/Applications/ABCD-UUID/Documents/x.dat").unwrap();
        assert!(!ro);
        assert!(host.ends_with("Documents/x.dat"));

        // System paths map read-only.
        let (_, ro) = sb.translate("/System/Library/Fonts/Helvetica.ttf").unwrap();
        assert!(ro);

        // Escapes are refused.
        assert!(sb
            .translate("/var/mobile/Applications/ABCD-UUID/../../../etc/passwd")
            .is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prefix_is_component_aware() {
        assert_eq!(strip_prefix_dir("/usr/lib/x", "/usr"), Some("lib/x"));
        assert_eq!(strip_prefix_dir("/usrlocal/x", "/usr"), None);
        assert_eq!(strip_prefix_dir("/usr", "/usr"), Some(""));
    }
}
