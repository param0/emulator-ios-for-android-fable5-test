//! The `Machine`: owns the guest process and drives the execute/dispatch loop.

use ios_emu_common::{EmuResult, GuestAddr, PAGE_SIZE};
use ios_emu_loader::dyld::TrampolineTable;
use ios_emu_loader::MachOImage;
use ios_emu_memory::GuestMemory;
use ios_emu_vfs::Sandbox;

use crate::cpu::{CpuContext, ExecEvent, Executor};
use crate::env::{CallContext, Dispatch, HostEnvironment, NullEnvironment};
use crate::fd::FdTable;
use crate::loader::{build_process, LayoutConfig};
use crate::syscall::{SyscallCtx, SyscallDispatcher};

/// Tunables for the process address-space layout. The defaults keep the image,
/// heap, stack, anonymous arena, and trampoline page in disjoint ranges of the
/// low 8 GiB (see the module docs in `memory`).
#[derive(Clone)]
pub struct MachineConfig {
    pub heap_base: GuestAddr,
    pub heap_size: u64,
    pub stack_top: GuestAddr,
    pub stack_size: u64,
    /// Host-only fallback base for the trampoline/stub page. On device this is
    /// ignored: the region is reserved at an OS-chosen base via
    /// `reserve_native_region` so the later `mprotect` to r-x targets real
    /// memory.
    pub trampoline_base: GuestAddr,
    /// Host-only fallback ASLR slide (0 = map at preferred). On device the slide
    /// is derived from the image reservation base.
    pub slide: u64,
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig {
            heap_base: GuestAddr(0x0000_0001_2000_0000),
            heap_size: 64 * 1024 * 1024,
            stack_top: GuestAddr(0x0000_0001_C000_0000),
            stack_size: 8 * 1024 * 1024,
            trampoline_base: GuestAddr(0x0000_0005_0000_0000),
            slide: 0,
        }
    }
}

/// Why the process stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: i32,
}

/// A fully-constructed guest process ready to run.
pub struct Machine {
    pub cpu: CpuContext,
    pub mem: GuestMemory,
    pub vfs: Sandbox,
    pub fds: FdTable,
    pub dispatcher: SyscallDispatcher,
    world: Box<dyn HostEnvironment>,
    trampolines: TrampolineTable,
    tramp_range: (u64, u64),
    exit: Option<i32>,
    /// Module initializers to run before `main` (in registration order).
    pub init_funcs: Vec<GuestAddr>,
    pub entry: GuestAddr,
    /// Argument registers for entry into `main`.
    arg_regs: [u64; 4],
    sp: u64,
}

impl Machine {
    /// Capacity reserved for the trampoline/stub page on device (16 B/stub, so
    /// this holds ~64k distinct imports — far beyond any real binary).
    #[cfg(target_os = "android")]
    const TRAMPOLINE_RESERVE: u64 = 0x10_0000; // 1 MiB

    /// Construct a process from a Mach-O `bytes` image inside `vfs`, servicing
    /// HLE calls through `world`. Pass [`NullEnvironment`] to run with syscalls
    /// only (no frameworks).
    pub fn load(
        cfg: &MachineConfig,
        bytes: &[u8],
        vfs: Sandbox,
        world: Box<dyn HostEnvironment>,
    ) -> EmuResult<Machine> {
        let image = MachOImage::parse(bytes)?;
        log::info!(
            "loaded Mach-O: {} segments, {} dylibs, pie={}",
            image.segments.len(),
            image.dylibs.len(),
            image.is_pie()
        );

        let mut mem = GuestMemory::new(cfg.heap_base, cfg.heap_size)?;

        // ASLR slide. On device, reserve the image span at an OS-chosen base (no
        // MAP_FIXED — Android 16 blocks the preferred 0x100000000 range) and
        // derive `slide = dynamic_base - preferred_base`; on the host there is no
        // native reservation, so honour the configured slide (0 = map at
        // preferred). Both segment placement (`seg.vmaddr + slide`) and
        // `resolve_entry` relocate by this value, keeping them consistent.
        #[cfg(target_os = "android")]
        let slide = crate::loader::reserve_image_and_slide(&image)?.1;
        #[cfg(not(target_os = "android"))]
        let slide = cfg.slide;

        // Trampoline/stub page. It is written (BRK sleds), then `mprotect`ed to
        // r-x, so on device it MUST be backed by a real OS reservation — a
        // hardcoded address (0x500000000) that was never mmap'd makes the later
        // mprotect fail. Reserve an OS-chosen base for a fixed capacity; on the
        // host the configured address is fine (host-Vec-backed bookkeeping).
        #[cfg(target_os = "android")]
        let trampoline_base = crate::loader::reserve_native_region(Self::TRAMPOLINE_RESERVE)?;
        #[cfg(not(target_os = "android"))]
        let trampoline_base = cfg.trampoline_base;

        // Main-thread stack. The hardcoded 0x1bf800000 is not grantable on
        // Android 16, so the guest's first SP write faults SEGV_MAPERR. Reserve
        // real memory at an OS-chosen base (no MAP_FIXED) and set stack_top =
        // base + size so SP starts at the high end and grows down into mapped
        // pages. On the host the configured top is fine (Vec-backed bookkeeping).
        #[cfg(target_os = "android")]
        let stack_top = {
            let base = crate::loader::reserve_native_region(cfg.stack_size)?;
            GuestAddr(base.raw() + cfg.stack_size)
        };
        #[cfg(not(target_os = "android"))]
        let stack_top = cfg.stack_top;

        let layout = LayoutConfig {
            slide,
            stack_top,
            stack_size: cfg.stack_size,
            trampoline_base,
            trampoline_stride: 16,
            argv0: format!("{}/Executable", vfs.bundle_ios_path()),
        };
        let proc = build_process(&image, bytes, &mut mem, &layout)?;

        let cpu = CpuContext::entry(proc.entry.raw(), proc.sp.raw(), proc.arg_regs);

        Ok(Machine {
            cpu,
            mem,
            vfs,
            fds: FdTable::default(),
            dispatcher: SyscallDispatcher::new(),
            world,
            trampolines: proc.trampolines,
            tramp_range: (proc.trampoline_base.raw(), proc.trampoline_end.raw()),
            exit: None,
            init_funcs: proc.init_funcs,
            entry: proc.entry,
            arg_regs: proc.arg_regs,
            sp: proc.sp.raw(),
        })
    }

    /// Convenience constructor with the default [`NullEnvironment`].
    pub fn load_bare(cfg: &MachineConfig, bytes: &[u8], vfs: Sandbox) -> EmuResult<Machine> {
        Self::load(cfg, bytes, vfs, Box::new(NullEnvironment))
    }

    /// `true` if `addr` lands in the import-trampoline page.
    #[inline]
    pub fn is_trampoline(&self, addr: GuestAddr) -> bool {
        (self.tramp_range.0..self.tramp_range.1).contains(&addr.raw())
    }

    /// Notify the HLE layer that the image is mapped (runs before initializers).
    pub fn start_environment(&mut self) {
        let mut ctx = CallContext { cpu: &mut self.cpu, mem: &mut self.mem, vfs: &self.vfs };
        self.world.on_process_start(&mut ctx);
    }

    /// Run the execute/dispatch loop until the process exits or faults.
    ///
    /// The loop is backend-agnostic: `exec.resume` yields the next
    /// [`ExecEvent`], which we service and then loop. On device `exec` is the
    /// native AArch64 backend; in tests it is a [`crate::cpu::ScriptedExecutor`].
    pub fn run(&mut self, exec: &mut dyn Executor) -> EmuResult<ProcessExit> {
        loop {
            match exec.resume(&mut self.cpu) {
                ExecEvent::Syscall { .. } => {
                    self.service_syscall();
                    if let Some(code) = self.exit {
                        return Ok(ProcessExit { code });
                    }
                }
                ExecEvent::TrampolineCall { addr } => {
                    self.service_trampoline(addr);
                }
                ExecEvent::Exited { code } => {
                    return Ok(ProcessExit { code });
                }
                ExecEvent::Breakpoint { imm } => {
                    log::error!("guest BRK #{imm} at {}", self.cpu.pc_addr());
                    return Ok(ProcessExit { code: 128 + 5 });
                }
                ExecEvent::Fault { pc, reason } => {
                    return Err(ios_emu_common::EmuError::Memory {
                        addr: pc.raw(),
                        reason,
                    });
                }
            }
        }
    }

    fn service_syscall(&mut self) {
        let mut ctx = SyscallCtx {
            cpu: &mut self.cpu,
            mem: &mut self.mem,
            vfs: &self.vfs,
            fds: &mut self.fds,
            exit: &mut self.exit,
        };
        self.dispatcher.dispatch(&mut ctx);
    }

    /// Service a call that branched into the trampoline page: identify the
    /// imported symbol, offer it to the HLE environment, fall back to a logged
    /// stub, then return to the caller (`pc = lr`).
    fn service_trampoline(&mut self, addr: GuestAddr) {
        let symbol = match self.trampolines.get(&addr.raw()) {
            Some(s) => s.clone(),
            None => {
                log::warn!("call into unmapped trampoline {addr}");
                self.cpu.pc = self.cpu.lr;
                return;
            }
        };

        // Disjoint field borrows: the environment gets cpu/mem/vfs while we hold
        // `&mut self.world` — the borrow checker sees these as separate fields.
        let mut ctx = CallContext { cpu: &mut self.cpu, mem: &mut self.mem, vfs: &self.vfs };
        let disp = self.world.call(&symbol, &mut ctx);
        if disp == Dispatch::Unhandled {
            log::trace!(target: "ios_emu::stub", "STUB import::{symbol}");
            self.cpu.set_ret(0);
        }
        // Return to caller.
        self.cpu.pc = self.cpu.lr;
    }

    /// Reset the register file to the process entry state (used to re-run, or to
    /// invoke an initializer with a landing pad as `lr`).
    pub fn reset_to_entry(&mut self) {
        self.cpu = CpuContext::entry(self.entry.raw(), self.sp, self.arg_regs);
    }
}

/// The 16 KiB page size assumption is load-bearing for the layout; assert it at
/// compile time so a mis-set constant fails the build rather than at runtime.
const _: () = assert!(PAGE_SIZE == 0x4000);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::ScriptedExecutor;
    use ios_emu_common::Protection;
    use ios_emu_memory::RegionKind;

    /// Build a Machine directly (bypassing Mach-O parsing) to unit-test the run
    /// loop's syscall dispatch.
    fn bare_machine() -> Machine {
        let root = std::env::temp_dir().join(format!("ios-emu-machine-{}", std::process::id()));
        let vfs = Sandbox::prepare(&root, &root.join("sys"), "T", "UUID").unwrap();
        let mut mem = GuestMemory::new(GuestAddr(0x1_2000_0000), 0x10000).unwrap();
        mem.map(GuestAddr(0x1_0000_0000), 0x4000, Protection::rw(), RegionKind::Stack, "scratch")
            .unwrap();
        Machine {
            cpu: CpuContext::default(),
            mem,
            vfs,
            fds: FdTable::default(),
            dispatcher: SyscallDispatcher::new(),
            world: Box::new(NullEnvironment),
            trampolines: TrampolineTable::new(),
            tramp_range: (0, 0),
            exit: None,
            init_funcs: Vec::new(),
            entry: GuestAddr(0),
            arg_regs: [0; 4],
            sp: 0,
        }
    }

    #[test]
    fn exit_syscall_stops_the_loop() {
        let mut m = bare_machine();
        // SYS_exit(42): x16 = 1, x0 = 42.
        m.cpu.x[16] = 1;
        m.cpu.x[0] = 42;
        let mut exec = ScriptedExecutor::new([ExecEvent::Syscall { imm: 0x80 }]);
        let exit = m.run(&mut exec).unwrap();
        assert_eq!(exit.code, 42);
    }

    #[test]
    fn write_syscall_reports_length() {
        let mut m = bare_machine();
        // Put a string in scratch memory and issue write(1, buf, len).
        let buf = GuestAddr(0x1_0000_0000);
        m.mem.write(buf, b"hi").unwrap();
        m.cpu.x[16] = 4; // SYS_write
        m.cpu.x[0] = 1; // stdout
        m.cpu.x[1] = buf.raw();
        m.cpu.x[2] = 2;
        // After write, dispatch sets x0; then exit to end the loop.
        let mut exec = ScriptedExecutor::new([
            ExecEvent::Syscall { imm: 0x80 },
            ExecEvent::Exited { code: 0 },
        ]);
        m.run(&mut exec).unwrap();
        assert_eq!(m.cpu.x[0], 2);
    }
}
