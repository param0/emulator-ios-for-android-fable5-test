//! Parsed representation of a thin arm64 Mach-O image.

use super::consts::*;
use super::reader::Reader;
use ios_emu_common::{EmuError, EmuResult, Protection};

/// A `__TEXT`/`__DATA`/... segment (`LC_SEGMENT_64`).
#[derive(Clone, Debug)]
pub struct Segment {
    pub name: String,
    /// Preferred virtual address (pre-slide).
    pub vmaddr: u64,
    pub vmsize: u64,
    /// Offset of the segment's bytes within the file.
    pub fileoff: u64,
    pub filesize: u64,
    pub initprot: Protection,
    pub maxprot: Protection,
    pub sections: Vec<Section>,
}

/// A section within a segment.
#[derive(Clone, Debug)]
pub struct Section {
    pub seg_name: String,
    pub name: String,
    pub addr: u64,
    pub size: u64,
    pub offset: u32,
    pub flags: u32,
}

impl Section {
    /// `true` for `S_ZEROFILL`/BSS sections that have no file backing.
    pub fn is_zerofill(&self) -> bool {
        (self.flags & SECTION_TYPE) == S_ZEROFILL
    }

    /// `true` for module init-function pointer arrays (C++/ObjC `+load`,
    /// `__attribute__((constructor))`). The loader hands these to `core` to run
    /// before `main`.
    pub fn is_mod_init(&self) -> bool {
        (self.flags & SECTION_TYPE) == S_MOD_INIT_FUNC_POINTERS
    }
}

/// A dylib dependency (`LC_LOAD_DYLIB` and friends).
#[derive(Clone, Debug)]
pub struct Dylib {
    pub path: String,
    pub weak: bool,
    pub reexport: bool,
}

/// Where execution begins.
#[derive(Clone, Copy, Debug)]
pub enum EntryPoint {
    /// `LC_MAIN`: file offset of `main` from the mach header. Runtime address is
    /// `mach_header_addr + file_offset`. `stack_size` is a hint (may be 0).
    Main { file_offset: u64, stack_size: u64 },
    /// `LC_UNIXTHREAD`: absolute (pre-slide) PC from the thread state.
    Thread { vmaddr: u64 },
}

/// `LC_ENCRYPTION_INFO_64` — FairPlay DRM range. iOS App Store binaries ship
/// with `__TEXT` encrypted; the loader surfaces this so the pipeline can refuse
/// (or, for a jailbroken/decrypted dump, confirm `cryptid == 0`).
#[derive(Clone, Copy, Debug)]
pub struct Encryption {
    pub cryptoff: u32,
    pub cryptsize: u32,
    pub cryptid: u32,
}

/// Location of the classic symbol table (`LC_SYMTAB`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SymtabInfo {
    pub symoff: u32,
    pub nsyms: u32,
    pub stroff: u32,
    pub strsize: u32,
}

/// Location of dyld's compressed bind/rebase/export info (`LC_DYLD_INFO_ONLY`).
#[derive(Clone, Copy, Debug, Default)]
pub struct DyldInfo {
    pub rebase_off: u32,
    pub rebase_size: u32,
    pub bind_off: u32,
    pub bind_size: u32,
    pub lazy_bind_off: u32,
    pub lazy_bind_size: u32,
    pub export_off: u32,
    pub export_size: u32,
}

/// Fully-parsed image description consumed by the `core` crate.
#[derive(Debug)]
pub struct MachOImage {
    pub cpu_subtype: u32,
    pub filetype: u32,
    pub flags: u32,
    pub segments: Vec<Segment>,
    pub dylibs: Vec<Dylib>,
    pub dylinker: Option<String>,
    pub entry: Option<EntryPoint>,
    pub symtab: Option<SymtabInfo>,
    pub dyld_info: Option<DyldInfo>,
    pub encryption: Option<Encryption>,
    pub uuid: Option<[u8; 16]>,
    pub function_starts: Option<(u32, u32)>,
    pub min_os_version: Option<u32>,
}

impl MachOImage {
    /// `true` for a position-independent executable (ASLR-eligible).
    pub fn is_pie(&self) -> bool {
        self.flags & MH_PIE != 0
    }

    /// Preferred base of the `__TEXT` segment (== address the mach header maps
    /// to before slide). Used to compute the ASLR slide in `core`.
    pub fn text_vmaddr(&self) -> u64 {
        self.segments
            .iter()
            .find(|s| s.name == "__TEXT")
            .map(|s| s.vmaddr)
            .unwrap_or(0)
    }

    /// `true` if `__TEXT` is still FairPlay-encrypted (`cryptid != 0`).
    pub fn is_encrypted(&self) -> bool {
        self.encryption.map(|e| e.cryptid != 0).unwrap_or(false)
    }

    /// Parse a Mach-O or FAT container, selecting the arm64(e) slice.
    pub fn parse(bytes: &[u8]) -> EmuResult<MachOImage> {
        let magic = Reader::new(bytes).u32_le()?;
        let (slice, off) = match magic {
            FAT_MAGIC | FAT_MAGIC_64 => Self::select_fat_slice(bytes)?,
            MH_MAGIC_64 => (bytes, 0),
            MH_CIGAM_64 => {
                return Err(EmuError::MachO("big-endian Mach-O is not supported".into()))
            }
            other => return Err(EmuError::MachO(format!("unknown magic {other:#010x}"))),
        };
        Self::parse_thin(slice, off)
    }

    /// Locate the arm64 (preferring arm64e) slice inside a universal binary and
    /// return `(bytes_of_whole_file, offset_of_slice)`.
    fn select_fat_slice(bytes: &[u8]) -> EmuResult<(&[u8], usize)> {
        let mut r = Reader::new(bytes);
        let magic = r.u32_be()?; // FAT fields are big-endian
        let is64 = magic == FAT_MAGIC_64;
        let nfat = r.u32_be()?;
        let mut best: Option<(u64, u64, u32)> = None; // (off, size, subtype)
        for _ in 0..nfat {
            let cputype = r.u32_be()?;
            let cpusubtype = r.u32_be()?;
            let (offset, size) = if is64 {
                let o = r.u64_be()?;
                let s = r.u64_be()?;
                let _align = r.u32_be()?;
                let _reserved = r.u32_be()?;
                (o, s)
            } else {
                let o = r.u32_be()? as u64;
                let s = r.u32_be()? as u64;
                let _align = r.u32_be()?;
                (o, s)
            };
            if cputype == CPU_TYPE_ARM64 {
                // Prefer arm64e over generic arm64 when both are present.
                let better = best.map(|(_, _, st)| cpusubtype == CPU_SUBTYPE_ARM64E && st != CPU_SUBTYPE_ARM64E);
                if best.is_none() || better == Some(true) {
                    best = Some((offset, size, cpusubtype));
                }
            }
        }
        let (off, size, _) = best
            .ok_or_else(|| EmuError::MachO("universal binary has no arm64 slice".into()))?;
        let off = off as usize;
        let end = off
            .checked_add(size as usize)
            .ok_or_else(|| EmuError::MachO("fat slice overflow".into()))?;
        if end > bytes.len() {
            return Err(EmuError::MachO("fat slice extends past file".into()));
        }
        Ok((bytes, off))
    }

    fn parse_thin(bytes: &[u8], base: usize) -> EmuResult<MachOImage> {
        let mut r = Reader::at(bytes, base)?;
        let magic = r.u32_le()?;
        if magic != MH_MAGIC_64 {
            return Err(EmuError::MachO(format!("slice magic {magic:#010x} is not 64-bit Mach-O")));
        }
        let cputype = r.u32_le()?;
        if cputype != CPU_TYPE_ARM64 {
            return Err(EmuError::MachO(format!("cputype {cputype:#x} is not arm64")));
        }
        let cpu_subtype = r.u32_le()?;
        let filetype = r.u32_le()?;
        let ncmds = r.u32_le()?;
        let _sizeofcmds = r.u32_le()?;
        let flags = r.u32_le()?;
        let _reserved = r.u32_le()?;

        let mut img = MachOImage {
            cpu_subtype,
            filetype,
            flags,
            segments: Vec::new(),
            dylibs: Vec::new(),
            dylinker: None,
            entry: None,
            symtab: None,
            dyld_info: None,
            encryption: None,
            uuid: None,
            function_starts: None,
            min_os_version: None,
        };

        // Load commands begin immediately after the 32-byte mach_header_64.
        let mut cmd_off = base + 32;
        for i in 0..ncmds {
            let mut cr = Reader::at(bytes, cmd_off)?;
            let cmd = cr.u32_le()?;
            let cmdsize = cr.u32_le()? as usize;
            if cmdsize < 8 {
                return Err(EmuError::MachO(format!("load command {i} has bogus size {cmdsize}")));
            }
            img.parse_command(cmd, bytes, cmd_off, base)?;
            cmd_off = cmd_off
                .checked_add(cmdsize)
                .ok_or_else(|| EmuError::MachO("load-command offset overflow".into()))?;
        }

        if img.segments.is_empty() {
            return Err(EmuError::MachO("no LC_SEGMENT_64 commands".into()));
        }
        Ok(img)
    }

    /// Dispatch a single load command. `cmd_off` is the absolute file offset of
    /// the command; `slice_base` is the offset of the thin slice (non-zero for
    /// FAT), needed to convert file offsets that are relative to the slice.
    fn parse_command(
        &mut self,
        cmd: u32,
        bytes: &[u8],
        cmd_off: usize,
        _slice_base: usize,
    ) -> EmuResult<()> {
        match cmd {
            LC_SEGMENT_64 => self.parse_segment(bytes, cmd_off)?,
            LC_LOAD_DYLIB => self.parse_dylib(bytes, cmd_off, false, false)?,
            LC_LOAD_WEAK_DYLIB => self.parse_dylib(bytes, cmd_off, true, false)?,
            LC_REEXPORT_DYLIB => self.parse_dylib(bytes, cmd_off, false, true)?,
            LC_LOAD_DYLINKER => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                let name_off = r.u32_le()? as usize;
                self.dylinker = Some(read_lc_str(bytes, cmd_off, name_off)?);
            }
            LC_MAIN => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                let file_offset = r.u64_le()?;
                let stack_size = r.u64_le()?;
                self.entry = Some(EntryPoint::Main { file_offset, stack_size });
            }
            LC_UNIXTHREAD => {
                if let Some(pc) = parse_arm64_thread_pc(bytes, cmd_off)? {
                    // LC_MAIN takes precedence if both somehow appear.
                    if self.entry.is_none() {
                        self.entry = Some(EntryPoint::Thread { vmaddr: pc });
                    }
                }
            }
            LC_SYMTAB => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                self.symtab = Some(SymtabInfo {
                    symoff: r.u32_le()?,
                    nsyms: r.u32_le()?,
                    stroff: r.u32_le()?,
                    strsize: r.u32_le()?,
                });
            }
            LC_DYLD_INFO | LC_DYLD_INFO_ONLY => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                self.dyld_info = Some(DyldInfo {
                    rebase_off: r.u32_le()?,
                    rebase_size: r.u32_le()?,
                    bind_off: r.u32_le()?,
                    bind_size: r.u32_le()?,
                    lazy_bind_off: r.u32_le()?,
                    lazy_bind_size: r.u32_le()?,
                    export_off: r.u32_le()?,
                    export_size: r.u32_le()?,
                });
            }
            LC_ENCRYPTION_INFO_64 => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                self.encryption = Some(Encryption {
                    cryptoff: r.u32_le()?,
                    cryptsize: r.u32_le()?,
                    cryptid: r.u32_le()?,
                });
            }
            LC_UUID => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                let raw = r.bytes(16)?;
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(raw);
                self.uuid = Some(uuid);
            }
            LC_FUNCTION_STARTS => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                let off = r.u32_le()?;
                let size = r.u32_le()?;
                self.function_starts = Some((off, size));
            }
            LC_VERSION_MIN_IPHONEOS => {
                let mut r = Reader::at(bytes, cmd_off + 8)?;
                self.min_os_version = Some(r.u32_le()?);
            }
            _ => { /* Unhandled commands are skipped; cmdsize advances the cursor. */ }
        }
        Ok(())
    }

    fn parse_segment(&mut self, bytes: &[u8], cmd_off: usize) -> EmuResult<()> {
        // struct segment_command_64 layout after (cmd, cmdsize).
        let mut r = Reader::at(bytes, cmd_off + 8)?;
        let name = r.fixed_str(16)?;
        let vmaddr = r.u64_le()?;
        let vmsize = r.u64_le()?;
        let fileoff = r.u64_le()?;
        let filesize = r.u64_le()?;
        let maxprot = r.u32_le()?;
        let initprot = r.u32_le()?;
        let nsects = r.u32_le()?;
        let _flags = r.u32_le()?;

        let mut sections = Vec::with_capacity(nsects as usize);
        // section_64 records follow immediately; each is 80 bytes.
        for _ in 0..nsects {
            let sect_name = r.fixed_str(16)?;
            let seg_name = r.fixed_str(16)?;
            let addr = r.u64_le()?;
            let size = r.u64_le()?;
            let offset = r.u32_le()?;
            let _align = r.u32_le()?;
            let _reloff = r.u32_le()?;
            let _nreloc = r.u32_le()?;
            let flags = r.u32_le()?;
            let _reserved1 = r.u32_le()?;
            let _reserved2 = r.u32_le()?;
            let _reserved3 = r.u32_le()?;
            sections.push(Section { seg_name, name: sect_name, addr, size, offset, flags });
        }

        self.segments.push(Segment {
            name,
            vmaddr,
            vmsize,
            fileoff,
            filesize,
            initprot: Protection::from_vm_prot(initprot),
            maxprot: Protection::from_vm_prot(maxprot),
            sections,
        });
        Ok(())
    }

    fn parse_dylib(
        &mut self,
        bytes: &[u8],
        cmd_off: usize,
        weak: bool,
        reexport: bool,
    ) -> EmuResult<()> {
        // dylib_command: cmd, cmdsize, then struct dylib { name.offset, ... }.
        let mut r = Reader::at(bytes, cmd_off + 8)?;
        let name_off = r.u32_le()? as usize;
        let path = read_lc_str(bytes, cmd_off, name_off)?;
        self.dylibs.push(Dylib { path, weak, reexport });
        Ok(())
    }
}

/// Read a `lc_str` (NUL-terminated string embedded in a load command).
/// `name_off` is relative to the *start of the command*.
fn read_lc_str(bytes: &[u8], cmd_off: usize, name_off: usize) -> EmuResult<String> {
    let start = cmd_off
        .checked_add(name_off)
        .ok_or_else(|| EmuError::MachO("lc_str offset overflow".into()))?;
    if start >= bytes.len() {
        return Err(EmuError::MachO("lc_str offset past end".into()));
    }
    let tail = &bytes[start..];
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}

/// Extract the PC (`x[32]` in `arm_thread_state64_t`) from an `LC_UNIXTHREAD`
/// command, if it carries an ARM64 thread state.
fn parse_arm64_thread_pc(bytes: &[u8], cmd_off: usize) -> EmuResult<Option<u64>> {
    let mut r = Reader::at(bytes, cmd_off + 8)?;
    let flavor = r.u32_le()?;
    let count = r.u32_le()?;
    if flavor != ARM_THREAD_STATE64 || count != ARM_THREAD_STATE64_COUNT {
        return Ok(None);
    }
    // arm_thread_state64_t: x[29] (@0..232), fp(@232), lr(@240), sp(@248), pc(@256).
    r.skip(8 * 32)?; // skip x0..x28, fp, lr, sp -> 32 * u64
    let pc = r.u64_le()?;
    Ok(Some(pc))
}
