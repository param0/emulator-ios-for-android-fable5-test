//! AArch64 register context and the executor abstraction.

use ios_emu_common::GuestAddr;

/// A full AArch64 user-mode register file.
///
/// The layout deliberately mirrors `arm_thread_state64_t` so the native backend
/// can `memcpy` between this struct and the kernel thread state when suspending
/// / resuming a guest thread.
#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct CpuContext {
    /// General purpose registers x0..x28.
    pub x: [u64; 29],
    /// Frame pointer (x29).
    pub fp: u64,
    /// Link register (x30).
    pub lr: u64,
    /// Stack pointer.
    pub sp: u64,
    /// Program counter.
    pub pc: u64,
    /// NZCV condition flags (bits 31..28). The C (carry) flag doubles as the
    /// Darwin syscall error indicator on return.
    pub nzcv: u64,
    /// Thread pointer (`TPIDRRO_EL0`) — points at the emulated pthread/TLS block.
    pub tpidrro_el0: u64,
}

impl CpuContext {
    /// Build the register file for entry into a function: `pc`, `sp`, and the
    /// first four arguments in `x0..x3` (the C ABI registers `main` expects).
    pub fn entry(pc: u64, sp: u64, args: [u64; 4]) -> Self {
        let mut x = [0u64; 29];
        x[..4].copy_from_slice(&args);
        CpuContext { x, sp, pc, ..Default::default() }
    }

    /// Syscall / procedure argument register (`x0..x7`). Panics on `n > 7`,
    /// which would be a caller bug.
    #[inline]
    pub fn arg(&self, n: usize) -> u64 {
        assert!(n < 8, "AArch64 passes at most 8 args in registers");
        self.x[n]
    }

    /// Set the primary return value (`x0`).
    #[inline]
    pub fn set_ret(&mut self, v: u64) {
        self.x[0] = v;
    }

    /// Encode a Darwin syscall result: on error, set `x0 = errno` and the carry
    /// flag; on success, clear carry and set `x0 = value`. This matches the
    /// `libsystem_kernel` cerror convention the guest's syscall stubs expect.
    #[inline]
    pub fn set_syscall_result(&mut self, result: Result<u64, u32>) {
        const CARRY: u64 = 1 << 29; // NZCV: C is bit 29.
        match result {
            Ok(v) => {
                self.x[0] = v;
                self.nzcv &= !CARRY;
            }
            Err(errno) => {
                self.x[0] = errno as u64;
                self.nzcv |= CARRY;
            }
        }
    }

    #[inline]
    pub fn pc_addr(&self) -> GuestAddr {
        GuestAddr(self.pc)
    }
}

/// What made the executor return control to the dispatch loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecEvent {
    /// The guest executed `SVC #imm`. Darwin uses `#0x80` for BSD; the syscall
    /// number and class are taken from `x16`.
    Syscall { imm: u16 },
    /// The guest branched to `addr`, which lies inside the import trampoline
    /// page — i.e. it called an HLE'd library function. `lr` holds the return
    /// address to resume at.
    TrampolineCall { addr: GuestAddr },
    /// The guest executed `BRK #imm` (debug trap / `__builtin_trap`).
    Breakpoint { imm: u16 },
    /// The guest returned to the loader-provided "process exit" landing pad.
    Exited { code: i32 },
    /// An unrecoverable fault (bad access, undefined instruction).
    Fault { pc: GuestAddr, reason: &'static str },
}

/// Abstraction over the instruction-stepping backend.
///
/// `resume` runs guest code from the current [`CpuContext::pc`] until the next
/// [`ExecEvent`], mutating `cpu` in place (and, for the native backend, reading
/// and writing the shared guest memory that the executor mirrors).
pub trait Executor {
    fn resume(&mut self, cpu: &mut CpuContext) -> ExecEvent;
}

// FFI declaration of the native AArch64 backend implemented in
// `native/src/jit_exec.c`. It installs the register file, drops to EL0 guest
// execution, and traps back on `SVC`/trampoline/`BRK`. The Rust side owns the
// `CpuContext`; the C side receives a pointer to it. Compiled only for Android
// device targets; on the host it is stubbed out (see `NativeExecutor`).
#[cfg(target_os = "android")]
extern "C" {
    // Returns an `ExecEvent` discriminant; out-params carry the payload.
    fn ios_emu_native_resume(
        ctx: *mut CpuContext,
        out_imm: *mut u16,
        out_addr: *mut u64,
        out_code: *mut i32,
    ) -> u32;
}

/// Native, on-device executor. Bridges to the C backend over FFI.
pub struct NativeExecutor {
    _private: (),
}

impl NativeExecutor {
    pub fn new() -> Self {
        NativeExecutor { _private: () }
    }
}

impl Default for NativeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for NativeExecutor {
    #[cfg(target_os = "android")]
    #[allow(unsafe_code)] // the sole FFI call into the native backend
    fn resume(&mut self, cpu: &mut CpuContext) -> ExecEvent {
        let mut imm: u16 = 0;
        let mut addr: u64 = 0;
        let mut code: i32 = 0;
        // SAFETY: the C backend only reads/writes through the provided pointers,
        // and `cpu` is a valid, uniquely-borrowed `CpuContext`.
        let disc = unsafe {
            ios_emu_native_resume(cpu as *mut CpuContext, &mut imm, &mut addr, &mut code)
        };
        match disc {
            0 => ExecEvent::Syscall { imm },
            1 => ExecEvent::TrampolineCall { addr: GuestAddr(addr) },
            2 => ExecEvent::Breakpoint { imm },
            3 => ExecEvent::Exited { code },
            _ => ExecEvent::Fault { pc: cpu.pc_addr(), reason: "native backend fault" },
        }
    }

    #[cfg(not(target_os = "android"))]
    fn resume(&mut self, cpu: &mut CpuContext) -> ExecEvent {
        // The native backend is only available on device. On the host we surface
        // a fault so misconfiguration is obvious rather than silently hanging.
        ExecEvent::Fault { pc: cpu.pc_addr(), reason: "native executor unavailable on host" }
    }
}

/// A deterministic executor that replays a scripted queue of events. Used by the
/// host test-suite to exercise [`crate::machine::Machine::run`]'s dispatch logic
/// without a real CPU backend.
pub struct ScriptedExecutor {
    events: std::collections::VecDeque<ExecEvent>,
}

impl ScriptedExecutor {
    pub fn new(events: impl IntoIterator<Item = ExecEvent>) -> Self {
        ScriptedExecutor { events: events.into_iter().collect() }
    }
}

impl Executor for ScriptedExecutor {
    fn resume(&mut self, cpu: &mut CpuContext) -> ExecEvent {
        self.events
            .pop_front()
            .unwrap_or(ExecEvent::Fault { pc: cpu.pc_addr(), reason: "script exhausted" })
    }
}
