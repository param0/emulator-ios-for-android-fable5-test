//! High-Level Emulation (HLE) of the iOS runtime and frameworks.
//!
//! Rather than mapping and running the real `Foundation`/`UIKit` dylibs, the HLE
//! layer *implements* the library surface the guest imports. It plugs into the
//! core via [`ios_emu_core::HostEnvironment`]: when the guest calls an imported
//! symbol, [`HleEnvironment::call`] looks the name up in a registry of shims and
//! services it natively.
//!
//! The design is a flat, data-driven registry so a new framework or function is
//! added by writing one shim `fn` and registering it — no changes to the core
//! loop or the dispatch path. Frameworks register themselves in
//! [`frameworks::install_all`].

#![forbid(unsafe_code)]

pub mod objc;
pub mod frameworks;

use std::collections::HashMap;

use ios_emu_core::{CallContext, Dispatch, HostEnvironment};

use objc::ObjcRuntime;

/// A native shim implementing one imported symbol. It receives the shared
/// Obj-C runtime (so message dispatch and C functions share class state) and the
/// per-call [`CallContext`] (registers + guest memory + sandbox).
pub type ShimFn = fn(&mut ObjcRuntime, &mut CallContext) -> Dispatch;

/// The HLE environment handed to the core. Owns the Obj-C runtime and the shim
/// registry.
pub struct HleEnvironment {
    runtime: ObjcRuntime,
    shims: HashMap<&'static str, ShimFn>,
}

impl HleEnvironment {
    /// Build an environment with every bundled framework installed.
    pub fn new() -> Self {
        let mut shims = HashMap::new();
        frameworks::install_all(&mut shims);
        HleEnvironment { runtime: ObjcRuntime::new(), shims }
    }

    /// Number of registered symbols (for diagnostics / tests).
    pub fn symbol_count(&self) -> usize {
        self.shims.len()
    }
}

impl Default for HleEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl HostEnvironment for HleEnvironment {
    fn call(&mut self, symbol: &str, ctx: &mut CallContext) -> Dispatch {
        match self.shims.get(symbol) {
            Some(shim) => shim(&mut self.runtime, ctx),
            None => Dispatch::Unhandled,
        }
    }

    fn on_process_start(&mut self, _ctx: &mut CallContext) {
        // Pre-register the root classes so `[NSObject class]` resolves even
        // before the app touches them.
        self.runtime.preload_root_classes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_populated() {
        let env = HleEnvironment::new();
        // Sanity: the core Obj-C + libSystem symbols are present.
        assert!(env.symbol_count() > 10);
    }
}
