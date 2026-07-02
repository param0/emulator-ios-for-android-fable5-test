//! The `Machine`: owns the guest process and drives the execute/dispatch loop.

use std::collections::HashMap;

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
    /// imported symbol name -> stub address (for `dyld_stub_binder`).
    symbol_stubs: HashMap<String, u64>,
    /// lazy-bind offset -> (symbol, la_symbol_ptr slot) (for `dyld_stub_binder`).
    lazy_binds: HashMap<u64, (String, GuestAddr)>,
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

        // Tell the native trap handler where the stub page lives so a BRK there
        // is dispatched as an imported call (see service_trampoline) instead of
        // surfacing as a fatal SIGTRAP.
        #[cfg(target_os = "android")]
        crate::loader::set_native_trampoline_range(proc.trampoline_base, proc.trampoline_end);

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
            symbol_stubs: proc.symbol_stubs,
            lazy_binds: proc.lazy_binds,
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
                log::warn!("call into unmapped trampoline {addr} (lr {:#018x})", self.cpu.lr);
                self.cpu.pc = self.cpu.lr;
                return;
            }
        };
        // Lazy binding: the game called dyld_stub_binder via __stub_helper. This
        // is an architecture-level trampoline, not a normal library call — handle
        // it specially (resolve, patch the pointer, tail-jump) and return.
        if symbol == "dyld_stub_binder" {
            self.dyld_stub_binder();
            return;
        }

        // Name the imported call as it happens, so the symbol behind a __stubs
        // BRK is identifiable at runtime in logcat (not just at load time).
        log::info!(target: "ios_emu::import", "call {symbol} (stub {addr}, from lr {:#018x})", self.cpu.lr);

        // Disjoint field borrows: the environment gets cpu/mem/vfs while we hold
        // `&mut self.world` — the borrow checker sees these as separate fields.
        let mut ctx = CallContext { cpu: &mut self.cpu, mem: &mut self.mem, vfs: &self.vfs };
        let disp = self.world.call(&symbol, &mut ctx);
        if disp == Dispatch::Unhandled {
            // Graceful: log the unimplemented import and return 0 rather than
            // leaving the guest to re-execute the BRK as a fatal SIGTRAP.
            log::warn!(target: "ios_emu::import", "unimplemented import {symbol}; returning 0");
            self.cpu.set_ret(0);
        }
        // Return to caller.
        self.cpu.pc = self.cpu.lr;
    }

    /// HLE of `dyld_stub_binder` — the lazy-binding trampoline.
    ///
    /// `__stub_helper` pushes two words before branching here: `[sp]` is the
    /// lazy-binding-info offset (loaded via `ldr w16, #imm`, zero-extended) and
    /// `[sp+8]` is the image cookie (unused). We look the offset up in the lazy
    /// table for the symbol and the `la_symbol_ptr` slot, resolve the symbol to
    /// its stub, patch the slot so subsequent calls bypass the binder, pop the
    /// two-word frame, and tail-jump to the stub. `x0..x8` / `q0..q7` (the callee
    /// arguments) are never touched, so they survive the binder call intact.
    fn dyld_stub_binder(&mut self) {
        let sp = self.cpu.sp;
        // Low 32 bits: the stub stores the offset with `ldr w16` (32-bit).
        let lazy_offset = match self.mem.read_u64(GuestAddr(sp)) {
            Ok(v) => v & 0xffff_ffff,
            Err(_) => {
                log::error!("dyld_stub_binder: unreadable sp {sp:#018x}");
                self.cpu.pc = self.cpu.lr;
                return;
            }
        };

        let (symbol, la_ptr) = match self.lazy_binds.get(&lazy_offset) {
            Some((s, p)) => (s.clone(), *p),
            None => {
                log::error!("dyld_stub_binder: no lazy bind at offset {lazy_offset:#x}");
                self.cpu.pc = self.cpu.lr;
                return;
            }
        };

        let resolved = match self.symbol_stubs.get(&symbol) {
            Some(&addr) => addr,
            None => {
                log::error!("dyld_stub_binder: unresolved lazy symbol {symbol}");
                self.cpu.pc = self.cpu.lr;
                return;
            }
        };

        log::info!(
            target: "ios_emu::import",
            "lazy-bind {symbol}: la_ptr {la_ptr} -> stub {resolved:#018x}"
        );

        // Patch the lazy pointer so future calls jump straight to the stub, and
        // publish it to the native page (la_symbol_ptr lives in rw __DATA).
        if self.mem.write_u64(la_ptr, resolved).is_ok() {
            let _ = self.mem.commit_to_native(la_ptr);
        } else {
            log::warn!("dyld_stub_binder: could not patch la_ptr {la_ptr}");
        }

        // Pop the frame __stub_helper pushed, then tail-jump to the resolved stub
        // (which re-enters as the real symbol). Arguments in x0..x8 are untouched.
        self.cpu.sp = sp.wrapping_add(16);
        self.cpu.pc = resolved;
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
            symbol_stubs: HashMap::new(),
            lazy_binds: HashMap::new(),
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
    fn dyld_stub_binder_resolves_patches_and_jumps() {
        let mut m = bare_machine();
        let sp = GuestAddr(0x1_0000_0000);
        let la_ptr = GuestAddr(0x1_0000_0100);
        let lazy_offset = 0x40u64;

        // __stub_helper pushed [lazy_offset, dyld_private].
        m.mem.write_u64(sp, lazy_offset).unwrap();
        m.mem.write_u64(GuestAddr(sp.raw() + 8), 0xdead_beef).unwrap();
        m.lazy_binds.insert(lazy_offset, ("objc_msgSend".to_string(), la_ptr));
        m.symbol_stubs.insert("objc_msgSend".to_string(), 0x5_0000_0000);

        // Arguments that must survive the binder untouched.
        m.cpu.x[0] = 0x1111;
        m.cpu.x[1] = 0x2222;
        m.cpu.sp = sp.raw();
        m.cpu.lr = 0x1234;

        m.dyld_stub_binder();

        assert_eq!(m.cpu.pc, 0x5_0000_0000); // tail-jump to the resolved stub
        assert_eq!(m.cpu.sp, sp.raw() + 16); // helper frame popped
        assert_eq!(m.mem.read_u64(la_ptr).unwrap(), 0x5_0000_0000); // lazy ptr patched
        assert_eq!(m.cpu.x[0], 0x1111); // args preserved
        assert_eq!(m.cpu.x[1], 0x2222);
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
