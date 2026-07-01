//! Emulation core: process construction, CPU/executor abstraction, and the
//! Darwin syscall + import-trampoline dispatch loop.
//!
//! ## Execution model
//!
//! Because the host is *also* AArch64, the design executes guest instructions
//! natively wherever possible. The [`cpu::Executor`] trait abstracts the actual
//! stepping engine:
//!
//!   * [`cpu::NativeExecutor`] (device) — resumes into guest code with the
//!     register file installed, running real AArch64 until the guest issues an
//!     `SVC` (syscall), branches into the trampoline page (imported function),
//!     or hits a `BRK`. Implemented in `native/src/jit_exec.c` and bound over
//!     FFI. Only privileged/undefined instructions require fixups.
//!   * [`cpu::ScriptedExecutor`] (host tests) — replays a queue of
//!     [`cpu::ExecEvent`]s so the dispatch logic can be tested without a CPU.
//!
//! Either way the *control loop* lives here in [`machine::Machine::run`]: it
//! pumps `ExecEvent`s and services them through the syscall table or the
//! [`env::HostEnvironment`] (the HLE frameworks), which keeps the policy in safe
//! Rust regardless of the execution backend.

// `deny` (not `forbid`) so the single, audited FFI call into the native backend
// can opt in with `#[allow(unsafe_code)]`; everything else stays unsafe-free.
#![cfg_attr(not(test), deny(unsafe_code))]

pub mod cpu;
pub mod env;
pub mod fd;
pub mod loader;
pub mod machine;
pub mod syscall;

pub use cpu::{CpuContext, ExecEvent, Executor, ScriptedExecutor};
pub use env::{CallContext, Dispatch, HostEnvironment, NullEnvironment};
pub use machine::{Machine, MachineConfig, ProcessExit};
