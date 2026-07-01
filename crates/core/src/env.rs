//! The seam between the core dispatch loop and the high-level-emulation layer.
//!
//! When the guest calls an imported library symbol (via an import trampoline),
//! the core hands the call to a [`HostEnvironment`]. The HLE crate implements
//! this trait, keeping its Obj-C runtime / framework state inside itself, and
//! operates on the guest through the [`CallContext`] it is given. The core never
//! depends on the HLE crate — only on this trait — which keeps the dependency
//! graph acyclic and lets the core build and be tested in isolation with the
//! [`NullEnvironment`].

use ios_emu_memory::GuestMemory;
use ios_emu_vfs::Sandbox;

use crate::cpu::CpuContext;

/// Everything a native library shim needs to service a guest call: the register
/// file (arguments in `x0..x7`, return via `set_ret`), guest memory, and the
/// sandbox for path translation.
pub struct CallContext<'a> {
    pub cpu: &'a mut CpuContext,
    pub mem: &'a mut GuestMemory,
    pub vfs: &'a Sandbox,
}

impl CallContext<'_> {
    /// Convenience: the `n`th integer argument.
    #[inline]
    pub fn arg(&self, n: usize) -> u64 {
        self.cpu.arg(n)
    }

    /// Convenience: set the return value in `x0`.
    #[inline]
    pub fn ret(&mut self, v: u64) {
        self.cpu.set_ret(v);
    }
}

/// Outcome of handing a symbol to the environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// The environment serviced the call and set the return value.
    Handled,
    /// The environment does not implement this symbol; the core should fall back
    /// to its generic stub (log + return 0).
    Unhandled,
}

/// Implemented by the HLE layer to service imported-symbol calls.
pub trait HostEnvironment {
    /// Service a call to `symbol`. Return [`Dispatch::Unhandled`] to defer to the
    /// core's generic stub.
    fn call(&mut self, symbol: &str, ctx: &mut CallContext) -> Dispatch;

    /// Optional hook invoked once after the process image is mapped but before
    /// `main`, so frameworks can install class tables, shared objects, etc.
    fn on_process_start(&mut self, _ctx: &mut CallContext) {}
}

/// A no-op environment: every symbol is unhandled. Lets the core crate build and
/// run its own tests without the HLE layer present.
pub struct NullEnvironment;

impl HostEnvironment for NullEnvironment {
    fn call(&mut self, _symbol: &str, _ctx: &mut CallContext) -> Dispatch {
        Dispatch::Unhandled
    }
}
