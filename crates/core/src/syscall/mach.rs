//! Mach trap handlers.
//!
//! Mach traps are the low-level kernel entry points behind the Mach IPC and VM
//! APIs. Full IPC emulation is out of scope for app bring-up, so most traps are
//! serviced with plausible constants (fake port names, `KERN_SUCCESS`). The two
//! that genuinely matter — `mach_vm_allocate`/`deallocate` — are wired to the
//! guest memory manager so allocator paths that bypass `mmap` still work.

use ios_emu_common::{GuestAddr, Protection};

use super::SyscallCtx;

// Trap numbers are the magnitudes of the negative `x16` selector.
const MACH_VM_ALLOCATE_TRAP: u32 = 10;
const MACH_VM_DEALLOCATE_TRAP: u32 = 12;
const MACH_REPLY_PORT: u32 = 26;
const THREAD_SELF_TRAP: u32 = 27;
const TASK_SELF_TRAP: u32 = 28;
const HOST_SELF_TRAP: u32 = 29;
const MACH_MSG_TRAP: u32 = 31;
const MACH_MSG_OVERWRITE_TRAP: u32 = 32;
const SEMAPHORE_SIGNAL_TRAP: u32 = 33;
const SEMAPHORE_WAIT_TRAP: u32 = 36;

// Synthetic, non-zero port names handed to the guest. Real Mach names are
// opaque; the guest only compares them for equality and passes them back.
const FAKE_TASK_PORT: u64 = 0x0000_0103;
const FAKE_HOST_PORT: u64 = 0x0000_0203;
const FAKE_THREAD_PORT: u64 = 0x0000_0303;
const FAKE_REPLY_PORT: u64 = 0x0000_0403;

const KERN_SUCCESS: u64 = 0;

/// Dispatch a Mach trap by magnitude. `None` => not implemented (stubbed by the
/// caller, returning `KERN_SUCCESS`).
pub fn handle(number: u32, ctx: &mut SyscallCtx) -> Option<Result<u64, u32>> {
    let r = match number {
        TASK_SELF_TRAP => Ok(FAKE_TASK_PORT),
        HOST_SELF_TRAP => Ok(FAKE_HOST_PORT),
        THREAD_SELF_TRAP => Ok(FAKE_THREAD_PORT),
        MACH_REPLY_PORT => Ok(FAKE_REPLY_PORT),
        MACH_MSG_TRAP | MACH_MSG_OVERWRITE_TRAP => Ok(KERN_SUCCESS),
        SEMAPHORE_SIGNAL_TRAP | SEMAPHORE_WAIT_TRAP => Ok(KERN_SUCCESS),
        MACH_VM_ALLOCATE_TRAP => vm_allocate(ctx),
        MACH_VM_DEALLOCATE_TRAP => Ok(KERN_SUCCESS),
        _ => return None,
    };
    Some(r)
}

/// `_kernelrpc_mach_vm_allocate_trap(target, vm_address_t *addr, size, flags)`.
fn vm_allocate(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let addr_out = GuestAddr(ctx.cpu.arg(1));
    let size = ctx.cpu.arg(2);
    match ctx.mem.mmap_anon(size, Protection::rw()) {
        Ok(base) => {
            // Write the allocated address back through the out-pointer.
            if ctx.mem.write_u64(addr_out, base.raw()).is_err() {
                return Ok(4); // KERN_INVALID_ADDRESS
            }
            Ok(KERN_SUCCESS)
        }
        Err(_) => Ok(3), // KERN_NO_SPACE
    }
}
