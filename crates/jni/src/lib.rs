//! Android-facing integration crate.
//!
//! Two layers live here:
//!   * [`session`] — a **pure-Rust** orchestration API (`scan` / `launch`) that
//!     wires IPA discovery, the sandbox, the loader, the emulation core, and the
//!     HLE frameworks together. It has no Android dependency and is unit-testable
//!     on the host.
//!   * `bridge` (feature `android`) — thin `extern "system"` JNI entry points
//!     that marshal Java strings to/from [`session`] and install the logcat
//!     logger. Built into `libios_emu_jni.so`.

pub mod session;

#[cfg(feature = "android")]
mod bridge;

pub use session::{scan, launch, AppInfo, LaunchOutcome};
