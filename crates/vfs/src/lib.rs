//! Virtual filesystem and per-app sandbox.
//!
//! Every emulated app gets an isolated container laid out like an iOS app
//! sandbox:
//!
//! ```text
//! <host_root>/Applications/<UUID>/
//! ├── <AppName>.app/     <- unzipped IPA payload (read-only to the guest)
//! ├── Documents/
//! ├── Library/
//! │   ├── Caches/
//! │   └── Preferences/
//! └── tmp/
//! ```
//!
//! The guest sees iOS-absolute paths (`/var/mobile/Applications/<UUID>/...`,
//! `/System/...`). [`Sandbox::translate`] maps those to real host paths inside
//! the container, rejecting any path that would escape it (`..`, absolute
//! symlink targets, etc.). The `core` syscall layer calls [`Sandbox::translate`]
//! before every `open`/`stat`/`unlink`.

#![forbid(unsafe_code)]

mod sandbox;
mod translate;

pub use sandbox::{Sandbox, SandboxLayout};
pub use translate::{normalize_ios_path, TranslationError};
