//! The single error type threaded through the whole emulator.
//!
//! We hand-roll the enum (instead of pulling in `thiserror`) to keep the leaf
//! crates dependency-light and their compile times low. Every fallible API in
//! the workspace returns [`EmuResult`].

use core::fmt;

/// Convenience alias for `Result<T, EmuError>`.
pub type EmuResult<T> = Result<T, EmuError>;

/// A structured, source-preserving error covering every failure domain in the
/// emulator. Variants are grouped by subsystem so a caller can `match` on the
/// class of failure (e.g. treat every `Vfs` error as a guest `EACCES`).
#[derive(Debug)]
pub enum EmuError {
    /// The Mach-O / FAT container was structurally invalid.
    MachO(String),
    /// Dynamic-linker resolution failed (missing dylib, unresolved symbol).
    Dyld(String),
    /// Guest memory access was out of bounds or violated protections.
    Memory { addr: u64, reason: &'static str },
    /// A syscall was invoked that we do not implement and cannot safely stub.
    UnsupportedSyscall { class: SyscallClass, number: u32 },
    /// Virtual filesystem / sandbox error (path escape, missing node, ...).
    Vfs(String),
    /// Info.plist / bundle metadata could not be parsed.
    Plist(String),
    /// Archive (IPA/zip) could not be read.
    Archive(String),
    /// Objective-C runtime error (unknown class/selector when strict).
    ObjC(String),
    /// Underlying host I/O error.
    Io(std::io::Error),
    /// Catch-all for invariant violations that indicate a bug in the emulator.
    Internal(String),
}

/// Which syscall ABI a number belongs to. Darwin multiplexes several classes
/// through `SVC #0`, distinguished by the high byte of the syscall number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallClass {
    /// Classic BSD/Unix syscalls (`0x2000000 | n`).
    Unix,
    /// Mach traps (negative `x16`, e.g. `mach_msg_trap`).
    Mach,
    /// `machdep` / platform-specific class (`0x3000000 | n`).
    MachDep,
    /// Diagnostics class (`0x4000000 | n`).
    Diag,
}

impl fmt::Display for SyscallClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SyscallClass::Unix => "unix",
            SyscallClass::Mach => "mach",
            SyscallClass::MachDep => "machdep",
            SyscallClass::Diag => "diag",
        };
        f.write_str(s)
    }
}

impl fmt::Display for EmuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmuError::MachO(m) => write!(f, "mach-o: {m}"),
            EmuError::Dyld(m) => write!(f, "dyld: {m}"),
            EmuError::Memory { addr, reason } => {
                write!(f, "memory fault @ {addr:#018x}: {reason}")
            }
            EmuError::UnsupportedSyscall { class, number } => {
                write!(f, "unsupported {class} syscall #{number}")
            }
            EmuError::Vfs(m) => write!(f, "vfs: {m}"),
            EmuError::Plist(m) => write!(f, "plist: {m}"),
            EmuError::Archive(m) => write!(f, "archive: {m}"),
            EmuError::ObjC(m) => write!(f, "objc: {m}"),
            EmuError::Io(e) => write!(f, "io: {e}"),
            EmuError::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for EmuError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EmuError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for EmuError {
    fn from(e: std::io::Error) -> Self {
        EmuError::Io(e)
    }
}
