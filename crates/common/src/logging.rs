//! Thin logging helpers.
//!
//! The `log` facade is used everywhere; the concrete sink is installed by the
//! JNI layer (an `android_logger`-style backend) or by tests (`env_logger`).
//! Syscall/framework stubs funnel through [`trace_stub`] so that unimplemented
//! surface area is greppable in a single `logcat` filter.

/// Log an invocation of a stubbed syscall or framework function at `trace`
/// level with a consistent, machine-parseable prefix (`STUB`), then let the
/// caller return a safe default. Centralising this keeps the "what did the app
/// actually touch" audit trail uniform.
#[macro_export]
macro_rules! stub {
    ($subsystem:literal, $name:expr) => {
        ::log::trace!(target: "ios_emu::stub", "STUB {}::{}", $subsystem, $name);
    };
    ($subsystem:literal, $name:expr, $($arg:tt)+) => {
        ::log::trace!(
            target: "ios_emu::stub",
            "STUB {}::{} {}", $subsystem, $name, format_args!($($arg)+)
        );
    };
}

/// Runtime-check helper used by the memory manager and loader. Unlike
/// `assert!`, this returns an [`crate::EmuError::Internal`] so a malformed
/// guest binary degrades to a recoverable error rather than aborting the app.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !($cond) {
            return Err($crate::EmuError::Internal(format!($($arg)+)));
        }
    };
}
