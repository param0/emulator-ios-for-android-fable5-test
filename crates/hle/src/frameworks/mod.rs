//! Modular framework registry.
//!
//! Each framework is a module exposing `install(&mut Registry)` that inserts its
//! shims. Adding coverage for a new function is a one-line registration; adding a
//! whole framework is one `mod` + one `install` call in [`install_all`]. Nothing
//! here touches the core dispatch loop.

use std::collections::HashMap;

use ios_emu_core::{CallContext, Dispatch};

use crate::objc::ObjcRuntime;
use crate::ShimFn;

pub mod coregraphics;
pub mod foundation;
pub mod gles;
pub mod libsystem;
pub mod objc_rt;
pub mod uikit;

/// The symbol -> shim registry type frameworks populate.
pub type Registry = HashMap<&'static str, ShimFn>;

/// Install every bundled framework's shims.
pub fn install_all(map: &mut Registry) {
    objc_rt::install(map);
    libsystem::install(map);
    foundation::install(map);
    uikit::install(map);
    coregraphics::install(map);
    gles::install(map);
}

/// A generic "log and return 0" shim. Used for the long tail of framework
/// functions whose absence must not crash the app but whose behaviour is not yet
/// modelled. The symbol name is not available inside a bare `fn`, so callers
/// wanting the name in the log should register a named wrapper instead; this one
/// keeps the audit trail at the call site via the trampoline stub logger.
pub fn stub_zero(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(0);
    Dispatch::Handled
}

/// Register `name` to the shared [`stub_zero`] shim.
pub fn register_stub(map: &mut Registry, name: &'static str) {
    map.insert(name, stub_zero);
}
