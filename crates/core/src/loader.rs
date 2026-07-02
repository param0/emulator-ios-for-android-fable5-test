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
        // W^X: load every segment as rw- — never request execute and write at
        // the same time. Executable segments (e.g. __TEXT r-x) are flipped to
        // their final protection in step 4, after file data and dyld binds have
        // landed. A simultaneous rwx mapping is rejected by Android 16 / modern
        // ARM64 kernels and faults with SEGV_ACCERR on first access.
        let load_prot = Protection::rw();
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
    // W^X: map the stub page rw- to write the BRK sleds, then flip to r-x below
    // — never rwx. (Writing while mapped r-x would also fail the abstraction's
    // protection check, silently leaving the page un-filled.)
    let tramp_bytes = align_up(resolver.used().max(PAGE_SIZE), PAGE_SIZE);
    mem.map(
        cfg.trampoline_base,
        tramp_bytes,
        Protection::rw(),
        RegionKind::Segment,
        "__stubs",
    )?;
    // Fill with BRK sleds so an unintended fall-through faults immediately.
    {
        let mut off = 0;
        while off < tramp_bytes {
            mem.write_u32(cfg.trampoline_base + off, AARCH64_BRK0)?;
            off += 4;
        }
    }
    // Now executable-only.
    mem.protect(cfg.trampoline_base, Protection::rx())?;
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

    // ---- 7. Entry point (slid) ---------------------------------------------
    let entry = resolve_entry(image, slide)?;

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

/// The image's preferred (link-time) base and the total span, in bytes, that
/// must be reserved to hold every segment. `__PAGEZERO` is excluded — it is an
/// unmapped guard, not real backing. The preferred base is `__TEXT`'s vmaddr
/// (0x100000000 for a PIE main executable).
pub fn image_span(image: &MachOImage) -> (u64, u64) {
    let preferred_base = image.text_vmaddr();
    let mut high = preferred_base;
    for seg in &image.segments {
        if seg.name == "__PAGEZERO" {
            continue;
        }
        high = high.max(seg.vmaddr.saturating_add(seg.vmsize));
    }
    let len = ios_emu_common::align_up(high.saturating_sub(preferred_base), PAGE_SIZE);
    (preferred_base, len)
}

/// Apply the ASLR `slide` to the image's entry point / thread PC.
///
/// For a PIE, `slide = dynamic_base - preferred_base`. Every link-time address
/// (segment vmaddrs, the mach-header address, the entry point, and an
/// `LC_UNIXTHREAD` PC) is relocated by adding `slide`. `LC_MAIN`'s `file_offset`
/// is relative to the mach header, which maps at `text_vmaddr + slide`.
pub fn resolve_entry(image: &MachOImage, slide: u64) -> EmuResult<GuestAddr> {
    let header_addr = image.text_vmaddr().wrapping_add(slide);
    match image.entry {
        Some(EntryPoint::Main { file_offset, .. }) => {
            Ok(GuestAddr(header_addr.wrapping_add(file_offset)))
        }
        Some(EntryPoint::Thread { vmaddr }) => Ok(GuestAddr(vmaddr.wrapping_add(slide))),
        None => Err(EmuError::MachO("image has no entry point".into())),
    }
}

/// Device path: reserve `len` bytes of real, OS-chosen memory via the C backend
/// (no `MAP_FIXED`) and return its base. Used for both the image span and the
/// trampoline/stub page — any region that must later be `mprotect`ed on device
/// has to be backed by a genuine reservation, or the `mprotect` fails.
#[cfg(target_os = "android")]
#[allow(unsafe_code)] // the sole FFI call into the native reservation backend
pub fn reserve_native_region(len: u64) -> EmuResult<GuestAddr> {
    extern "C" {
        // native/src/vm_bridge.c — aborts on failure, so a non-null return is
        // guaranteed here.
        fn ios_emu_native_map_region(len: usize) -> *mut core::ffi::c_void;
    }
    // SAFETY: FFI into our own allocator; the callee validates `len` and aborts
    // rather than returning MAP_FAILED.
    let base = unsafe { ios_emu_native_map_region(len as usize) } as u64;
    if base == 0 {
        return Err(EmuError::Memory { addr: 0, reason: "native reservation returned null" });
    }
    Ok(GuestAddr(base))
}

/// Device path: reserve the image span and compute the ASLR slide from the
/// returned base. Feeds `LayoutConfig::slide` so segment mapping and
/// [`resolve_entry`] relocate consistently.
#[cfg(target_os = "android")]
pub fn reserve_image_and_slide(image: &MachOImage) -> EmuResult<(GuestAddr, u64)> {
    let (preferred_base, len) = image_span(image);
    let base = reserve_native_region(len)?;
    let slide = base.raw().wrapping_sub(preferred_base);
    log::info!(
        "PIE image: dynamic base {} (preferred {preferred_base:#x}, slide {slide:#x})",
        base
    );
    Ok((base, slide))
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

#[cfg(test)]
mod tests {
    use super::*;
    use ios_emu_common::Protection;
    use ios_emu_loader::macho::image::{MachOImage, Segment};

    fn seg(name: &str, vmaddr: u64, vmsize: u64) -> Segment {
        Segment {
            name: name.into(),
            vmaddr,
            vmsize,
            fileoff: 0,
            filesize: vmsize,
            initprot: Protection::rw(),
            maxprot: Protection::rw(),
            sections: vec![],
        }
    }

    fn image(entry: EntryPoint) -> MachOImage {
        MachOImage {
            cpu_subtype: 0,
            filetype: 2,
            flags: 0x0020_0000, // MH_PIE
            segments: vec![
                seg("__PAGEZERO", 0, 0x1_0000_0000),
                seg("__TEXT", 0x1_0000_0000, 0x8000),
                seg("__DATA", 0x1_0000_8000, 0x4000),
            ],
            dylibs: vec![],
            dylinker: None,
            entry: Some(entry),
            symtab: None,
            dyld_info: None,
            encryption: None,
            uuid: None,
            function_starts: None,
            min_os_version: None,
        }
    }

    #[test]
    fn span_excludes_pagezero() {
        let (base, len) = image_span(&image(EntryPoint::Main { file_offset: 0, stack_size: 0 }));
        assert_eq!(base, 0x1_0000_0000);
        assert_eq!(len, 0xC000); // __TEXT (0x8000) + __DATA (0x4000)
    }

    #[test]
    fn slide_relocates_lc_main_entry() {
        let img = image(EntryPoint::Main { file_offset: 0x100, stack_size: 0 });
        // OS picked base 0x2_0000_0000; preferred is 0x1_0000_0000.
        let slide = 0x2_0000_0000u64.wrapping_sub(0x1_0000_0000);
        assert_eq!(resolve_entry(&img, slide).unwrap().raw(), 0x2_0000_0100);
    }

    #[test]
    fn slide_relocates_thread_pc() {
        let img = image(EntryPoint::Thread { vmaddr: 0x1_0019_2918 });
        let slide = 0x3_0000_0000u64.wrapping_sub(0x1_0000_0000);
        assert_eq!(resolve_entry(&img, slide).unwrap().raw(), 0x3_0019_2918);
    }
}
