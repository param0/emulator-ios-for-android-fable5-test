//! Darwin/XNU syscall translation layer.
//!
//! iOS multiplexes several syscall ABIs through `SVC #0x80`, selected by the
//! value in `x16`:
//!   * a **positive** number is a BSD/Unix syscall (`SYS_*`),
//!   * a **negative** number is a Mach trap (`mach_msg_trap`, ...),
//!   * numbers with a non-zero high byte carry an explicit *class* in bits
//!     24..31 (`SYSCALL_CLASS_*`), used by the `syscall()` fallback path.
//!
//! [`decode`] normalises all three forms into a `(SyscallClass, number)` pair,
//! and [`SyscallDispatcher::dispatch`] routes it. Unimplemented-but-harmless
//! syscalls are logged and given a safe default (`ENOSYS`/0) rather than
//! aborting — a hard requirement for booting real apps that probe for features.

pub mod errno;
mod mach;
mod numbers;
mod unix;

use ios_emu_common::error::SyscallClass;
use ios_emu_memory::GuestMemory;
use ios_emu_vfs::Sandbox;

use crate::cpu::CpuContext;
use crate::fd::FdTable;

/// Everything a syscall handler may touch, gathered so handlers borrow exactly
/// what they need without reaching back into [`crate::machine::Machine`].
pub struct SyscallCtx<'a> {
    pub cpu: &'a mut CpuContext,
    pub mem: &'a mut GuestMemory,
    pub vfs: &'a Sandbox,
    pub fds: &'a mut FdTable,
    /// Set by `exit`/`exit_group` to unwind the run loop.
    pub exit: &'a mut Option<i32>,
}

/// A syscall handler: reads arguments from `ctx.cpu`, performs the effect, and
/// returns `Ok(value)` or `Err(errno)`.
pub type Handler = fn(&mut SyscallCtx) -> Result<u64, u32>;

const SYSCALL_CLASS_SHIFT: u32 = 24;
const SYSCALL_CLASS_MASK: u32 = 0xff << SYSCALL_CLASS_SHIFT;
const SYSCALL_NUMBER_MASK: u32 = !SYSCALL_CLASS_MASK;

/// Normalise a raw `x16` selector into a class + number.
pub fn decode(x16: u64) -> (SyscallClass, u32) {
    let raw = x16 as i64;
    if raw < 0 {
        // Mach traps are encoded as negative numbers.
        return (SyscallClass::Mach, (-raw) as u32);
    }
    let raw = x16 as u32;
    if raw & SYSCALL_CLASS_MASK != 0 {
        let class = (raw & SYSCALL_CLASS_MASK) >> SYSCALL_CLASS_SHIFT;
        let number = raw & SYSCALL_NUMBER_MASK;
        let class = match class {
            1 => SyscallClass::Mach,
            2 => SyscallClass::Unix,
            3 => SyscallClass::MachDep,
            4 => SyscallClass::Diag,
            _ => SyscallClass::Unix,
        };
        (class, number)
    } else {
        (SyscallClass::Unix, raw)
    }
}

/// Routes decoded syscalls to their handlers. The table is a plain match in
/// [`unix::handle`]/[`mach::handle`]; wrapping it in a struct keeps room for
/// per-instance state (audit counters, per-app policy) later.
#[derive(Default)]
pub struct SyscallDispatcher {
    /// Count of syscalls serviced, for diagnostics.
    pub serviced: u64,
    /// Count of syscalls that fell through to the generic stub.
    pub stubbed: u64,
}

impl SyscallDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Service one syscall, taking the number from `ctx.cpu.x[16]` and writing
    /// the result back with the Darwin cerror convention.
    pub fn dispatch(&mut self, ctx: &mut SyscallCtx) {
        self.serviced += 1;
        let (class, number) = decode(ctx.cpu.x[16]);
        let result = match class {
            SyscallClass::Unix => unix::handle(number, ctx).unwrap_or_else(|| {
                self.stubbed += 1;
                stub_syscall(class, number);
                Err(errno::ENOSYS)
            }),
            SyscallClass::Mach => mach::handle(number, ctx).unwrap_or_else(|| {
                self.stubbed += 1;
                stub_syscall(class, number);
                // Mach traps return KERN_SUCCESS(0) on the "we don't care" path.
                Ok(0)
            }),
            SyscallClass::MachDep | SyscallClass::Diag => {
                self.stubbed += 1;
                stub_syscall(class, number);
                Ok(0)
            }
        };
        ctx.cpu.set_syscall_result(result);
    }
}

/// Log an unimplemented syscall in a uniform, greppable form.
fn stub_syscall(class: SyscallClass, number: u32) {
    log::trace!(target: "ios_emu::stub", "STUB syscall {class}#{number}");
}
