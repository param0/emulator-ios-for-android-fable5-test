//! Dynamic-linking support: dyld bind-info parsing and symbol resolution.
//!
//! In a high-level-emulation design the on-device dylibs (`libSystem`,
//! `Foundation`, `UIKit`, ...) are **not** mapped. Instead every imported
//! symbol is bound to a unique *trampoline* address inside a dedicated stub
//! page. When the guest branches to a trampoline, the executor recognises the
//! address, looks up the associated handler, and services the call natively
//! (see the `hle` and `core::syscall` crates). This module produces the raw
//! material for that: the list of [`BindRecord`]s the guest expects to be
//! filled in, and a [`SymbolResolver`] abstraction the linker drives.

pub mod bind;
pub mod linker;

pub use bind::{parse_bind_info, BindKind, BindRecord};
pub use linker::{StubResolver, SymbolResolver, TrampolineTable};
