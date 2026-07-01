//! Turns a parsed [`MachOImage`] into a live process image inside a
//! [`GuestMemory`]: maps segments, applies dyld binds against HLE trampolines,
//! lays out the initial stack, and computes the entry point.
//!
//! ## Slide policy
//!
//! We map every image at its *preferred* address (slide = 0). This keeps all
//! absolute pointers baked into the binary — rebased pointers, initializer
//! arrays, Obj-C metadata — valid without having to replay the rebase opcode
//! stream, which materially simplifies bring-up. A production build randomises
//! the slide and processes `LC_DYLD_INFO` rebases; the seam for that is
//! [`LayoutConfig::slide`].

use ios_emu_common::{align_up, EmuError, EmuResult, GuestAddr, Protection, PAGE_SIZE};
use ios_emu_loader::dyld::{parse_bind_info, StubResolver, SymbolResolver, TrampolineTable};
use ios_emu_loader::macho::image::EntryPoint;
use ios_emu_loader::MachOImage;
use ios_emu_memory::{GuestMemory, RegionKind};

/// Addresses and sizes governing where the process is laid out. Defaults live in
/// [`crate::machine::MachineConfig`].
pub struct LayoutConfig {
    pub slide: u64,
    pub stack_top: GuestAddr,
    pub stack_size: u64,
    pub trampoline_base: GuestAddr,
    pub trampoline_stride: u64,
    /// `argv[0]` / `apple[0]` — the guest-visible path of the executable.
    pub argv0: String,
}

/// The result of laying out a process: everything the [`crate::machine::Machine`]
/// needs to start executing.
pub struct ProcessImage {
    pub entry: GuestAddr,
    /// Initial stack pointer (already populated with argc/argv/envp/apple).
    pub sp: GuestAddr,
    /// C ABI argument registers for entry into `main` (`x0..x3`).
    pub arg_regs: [u64; 4],
    pub trampolines: TrampolineTable,
    pub trampoline_base: GuestAddr,
    pub trampoline_end: GuestAddr,
    /// Module initializers (`__mod_init_func`) to run before `main`.
    pub init_funcs: Vec<GuestAddr>,
}

/// `BRK #0` — the AArch64 trap the native backend intercepts inside the
/// trampoline page (it never actually executes; the executor recognises the
/// address range first).
const AARCH64_BRK0: u32 = 0xD420_0000;

pub fn build_process(
    image: &MachOImage,
    bytes: &[u8],
    mem: &mut GuestMemory,
    cfg: &LayoutConfig,
) -> EmuResult<ProcessImage> {
    if image.is_encrypted() {
        return Err(EmuError::Dyld(
            "executable is FairPlay-encrypted; supply a decrypted binary".into(),
        ));
    }

    let slide = cfg.slide;

    // ---- 1. Map segments (writable during load, real protections later) -----
    // Runtime base of each segment, indexed as they appear in the load commands
    // (bind records reference this index, including __PAGEZERO).
    let mut seg_runtime: Vec<Option<GuestAddr>> = Vec::with_capacity(image.segments.len());
    for seg in &image.segments {
        if seg.name == "__PAGEZERO" {
            seg_runtime.push(None); // guard region, intentionally unmapped
            continue;
        }
        let base = GuestAddr(seg.vmaddr + slide);
        let file_end = (seg.fileoff + seg.filesize) as usize;
        let data: &[u8] = if seg.filesize > 0 && file_end <= bytes.len() {
            &bytes[seg.fileoff as usize..file_end]
        } else {
            &[]
        };
        // Map writable so dyld binds can be applied; downgraded below.
        let load_prot = Protection(seg.initprot.0 | Protection::WRITE.0);
        mem.map_with_data(base, seg.vmsize, data, load_prot, RegionKind::Segment, &seg.name)?;
        seg_runtime.push(Some(base));
    }

    // ---- 2. Resolve imports and bind pointers to trampolines ---------------
    let mut resolver = StubResolver::new(cfg.trampoline_base, cfg.trampoline_stride);
    if let Some(info) = &image.dyld_info {
        // Both the eager and lazy bind streams point pointers at imports; we
        // resolve them all eagerly since there is no real lazy-binding stub.
        for (off, size) in [
            (info.bind_off, info.bind_size),
            (info.lazy_bind_off, info.lazy_bind_size),
        ] {
            if size == 0 {
                continue;
            }
            let end = (off + size) as usize;
            let stream = bytes
                .get(off as usize..end)
                .ok_or_else(|| EmuError::Dyld("bind stream out of range".into()))?;
            for rec in parse_bind_info(stream)? {
                let seg_base = match seg_runtime.get(rec.seg_index as usize).and_then(|o| *o) {
                    Some(b) => b,
                    None => {
                        log::warn!("bind targets unmapped segment {}", rec.seg_index);
                        continue;
                    }
                };
                let target = seg_base + rec.seg_offset;
                match resolver.resolve(&rec.symbol, rec.lib_ordinal, rec.weak) {
                    Some(tramp) => {
                        let value = (tramp.raw() as i64 + rec.addend) as u64;
                        if mem.write_u64(target, value).is_err() {
                            log::warn!("failed to write bind for {} @ {target}", rec.symbol);
                        }
                    }
                    None if rec.weak => {
                        let _ = mem.write_u64(target, 0); // unresolved weak -> NULL
                    }
                    None => log::warn!("unresolved symbol {}", rec.symbol),
                }
            }
        }
    }

    // ---- 3. Publish the trampoline page ------------------------------------
    let tramp_bytes = align_up(resolver.used().max(PAGE_SIZE), PAGE_SIZE);
    mem.map(
        cfg.trampoline_base,
        tramp_bytes,
        Protection::rx(),
        RegionKind::Segment,
        "__stubs",
    )?;
    // Fill with BRK sleds so an unintended fall-through faults immediately.
    {
        let mut off = 0;
        while off < tramp_bytes {
            let _ = mem.write_u32(cfg.trampoline_base + off, AARCH64_BRK0);
            off += 4;
        }
    }
    let trampoline_end = cfg.trampoline_base + tramp_bytes;

    // ---- 4. Restore real segment protections -------------------------------
    for (seg, base) in image.segments.iter().zip(&seg_runtime) {
        if let Some(base) = base {
            let prot = if seg.initprot == Protection::NONE {
                Protection::READ
            } else {
                seg.initprot
            };
            mem.protect(*base, prot)?;
        }
    }

    // ---- 5. Collect module initializers ------------------------------------
    let mut init_funcs = Vec::new();
    for seg in &image.segments {
        for sect in &seg.sections {
            if sect.is_mod_init() {
                let count = sect.size / 8;
                for i in 0..count {
                    let ptr_addr = GuestAddr(sect.addr + slide + i * 8);
                    if let Ok(fp) = mem.read_u64(ptr_addr) {
                        init_funcs.push(GuestAddr(fp));
                    }
                }
            }
        }
    }

    // ---- 6. Build the initial stack ----------------------------------------
    let stack_base = GuestAddr(cfg.stack_top.raw() - cfg.stack_size);
    mem.map(stack_base, cfg.stack_size, Protection::rw(), RegionKind::Stack, "stack")?;
    let (sp, arg_regs) = build_stack(mem, cfg)?;

    // ---- 7. Entry point ----------------------------------------------------
    let header_addr = image.text_vmaddr() + slide;
    let entry = match image.entry {
        Some(EntryPoint::Main { file_offset, .. }) => GuestAddr(header_addr + file_offset),
        Some(EntryPoint::Thread { vmaddr }) => GuestAddr(vmaddr + slide),
        None => return Err(EmuError::MachO("image has no entry point".into())),
    };

    Ok(ProcessImage {
        entry,
        sp,
        arg_regs,
        trampolines: resolver.into_table(),
        trampoline_base: cfg.trampoline_base,
        trampoline_end,
        init_funcs,
    })
}

/// Write `argv`/`envp`/`apple` onto the stack and return `(sp, [argc, argv,
/// envp, apple])`. We enter `main(argc, argv, envp, apple)` directly (HLE of
/// libdyld's `start`), so the four values also go into `x0..x3`.
fn build_stack(mem: &mut GuestMemory, cfg: &LayoutConfig) -> EmuResult<(GuestAddr, [u64; 4])> {
    // Place the argv0 string just below the top of the stack.
    let mut top = cfg.stack_top.raw() - 16;
    let path = cfg.argv0.as_bytes();
    let str_addr = GuestAddr((top - path.len() as u64 - 1) & !0xf);
    mem.write(str_addr, path)?;
    mem.write(str_addr + path.len() as u64, &[0])?;
    top = str_addr.raw();

    // Pointer arrays (all 8-byte aligned): argv[argc+1], envp[1], apple[2].
    // Layout downward: apple[], envp[], argv[].
    let apple = GuestAddr((top - 16) & !0xf);
    mem.write_u64(apple, str_addr.raw())?;
    mem.write_u64(apple + 8, 0)?;

    let envp = GuestAddr((apple.raw() - 8) & !0xf);
    mem.write_u64(envp, 0)?;

    let argv = GuestAddr((envp.raw() - 16) & !0xf);
    mem.write_u64(argv, str_addr.raw())?;
    mem.write_u64(argv + 8, 0)?;

    // Final sp: 16-byte aligned, below everything.
    let sp = GuestAddr(argv.raw() & !0xf);
    let arg_regs = [1, argv.raw(), envp.raw(), apple.raw()];
    Ok((sp, arg_regs))
}
