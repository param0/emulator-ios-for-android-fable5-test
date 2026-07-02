//! Guest address-space manager.
//!
//! [`GuestMemory`] is a sparse map of page-aligned [`Region`]s. Each region
//! owns a host-side backing buffer; guest virtual addresses are resolved to
//! `&[u8]`/`&mut [u8]` slices with bounds- and protection-checking on every
//! access. Typed little-endian accessors ([`GuestMemory::read_u64`] etc.)
//! cover the common cases the loader, syscall layer, and Obj-C runtime need.
//!
//! On device, the native executor (see `native/src/jit_exec.c`) maps the
//! executable regions with `PROT_EXEC` for direct AArch64 execution; this crate
//! remains the single source of truth for the *layout* and *contents*, and the
//! executor mirrors regions marked [`Protection::EXEC`].

// `deny` (not `forbid`) so the single, audited FFI call that applies real page
// protections on device can opt in with `#[allow(unsafe_code)]`; everything else
// stays unsafe-free.
#![cfg_attr(not(test), deny(unsafe_code))]

mod allocator;
mod region;

pub use allocator::BumpAllocator;
pub use region::{Region, RegionKind};

use ios_emu_common::{align_up, EmuError, EmuResult, GuestAddr, Protection, PAGE_SIZE};
use std::collections::BTreeMap;

// Native backend hooks (native/src/vm_bridge.c). On device, `protect` forwards
// the final protection to `mprotect` (so W^X is kernel-enforced) and, when the
// region becomes executable, flushes the I-cache so freshly-written code is
// coherent before execution — ARM64 I/D caches are not.
#[cfg(target_os = "android")]
extern "C" {
    fn ios_emu_native_protect(addr: u64, len: usize, prot: u32) -> i32;
    fn ios_emu_native_flush_icache(addr: u64, len: usize);
}

/// The full emulated address space for one iOS process.
///
/// Regions are keyed by their base address in a `BTreeMap`, giving O(log n)
/// range lookups via `range(..=addr).next_back()`.
pub struct GuestMemory {
    /// base address -> region
    regions: BTreeMap<u64, Region>,
    /// Next free address for anonymous `mmap`-style allocations. Grows down
    /// from the top of the low 4 GiB user region to stay clear of the image.
    mmap_cursor: u64,
    /// Managed heap for `malloc`/`brk` emulation.
    heap: BumpAllocator,
}

impl GuestMemory {
    /// The top of the anonymous-mapping arena. iOS 32/64 processes live well
    /// below this; we grow anonymous maps downward from here.
    const MMAP_TOP: u64 = 0x0000_0002_0000_0000;

    /// Create an empty address space with a heap arena of `heap_size` bytes
    /// mapped at `heap_base`.
    pub fn new(heap_base: GuestAddr, heap_size: u64) -> EmuResult<Self> {
        let mut mem = GuestMemory {
            regions: BTreeMap::new(),
            mmap_cursor: Self::MMAP_TOP,
            heap: BumpAllocator::new(heap_base, heap_size),
        };
        mem.map(
            heap_base,
            align_up(heap_size, PAGE_SIZE),
            Protection::rw(),
            RegionKind::Heap,
            "heap",
        )?;
        Ok(mem)
    }

    /// Map a fresh, zero-filled region. Fails if it would overlap an existing
    /// mapping. `size` is rounded up to a page.
    pub fn map(
        &mut self,
        base: GuestAddr,
        size: u64,
        prot: Protection,
        kind: RegionKind,
        name: &str,
    ) -> EmuResult<()> {
        let size = align_up(size.max(1), PAGE_SIZE);
        self.check_free(base, size)?;
        let region = Region::new_zeroed(base, size, prot, kind, name);
        log::debug!("map {name:>10} {base} +{size:#x} {prot:?}");
        self.regions.insert(base.raw(), region);
        Ok(())
    }

    /// Map a region initialised from `data` (used for Mach-O segment file
    /// contents). `data` may be shorter than `size`; the tail is zero-filled
    /// (BSS). `data` longer than `size` is an error.
    pub fn map_with_data(
        &mut self,
        base: GuestAddr,
        size: u64,
        data: &[u8],
        prot: Protection,
        kind: RegionKind,
        name: &str,
    ) -> EmuResult<()> {
        let size = align_up(size.max(data.len() as u64), PAGE_SIZE);
        self.check_free(base, size)?;
        let region = Region::new_from(base, size, data, prot, kind, name)?;
        log::debug!("map {name:>10} {base} +{size:#x} {prot:?} (file {} B)", data.len());
        self.regions.insert(base.raw(), region);
        Ok(())
    }

    /// Anonymous mapping analogous to `mmap(NULL, ...)`. Returns the chosen
    /// base address (allocated top-down from [`Self::MMAP_TOP`]).
    pub fn mmap_anon(&mut self, size: u64, prot: Protection) -> EmuResult<GuestAddr> {
        let size = align_up(size.max(1), PAGE_SIZE);
        let base = self
            .mmap_cursor
            .checked_sub(size)
            .ok_or(EmuError::Memory { addr: size, reason: "mmap arena exhausted" })?;
        let base = GuestAddr(base).align_down(PAGE_SIZE);
        self.map(base, size, prot, RegionKind::Anonymous, "mmap")?;
        self.mmap_cursor = base.raw();
        Ok(base)
    }

    /// Change the protection of the region *containing* `addr`. (A full
    /// implementation would split regions on sub-range `mprotect`; iOS binaries
    /// almost always `mprotect` whole segments, which this handles.)
    ///
    /// On device this forwards to the C backend's `mprotect` wrapper so the final
    /// protection (e.g. `r-x` for `__TEXT`) is enforced by the kernel — the
    /// mechanism that makes the W^X load sequence (map `rw-`, copy, then flip to
    /// `r-x`) actually take effect on the real pages.
    #[cfg_attr(target_os = "android", allow(unsafe_code))]
    pub fn protect(&mut self, addr: GuestAddr, prot: Protection) -> EmuResult<()> {
        let base = self.region_base(addr)?;
        let region = self.regions.get_mut(&base).unwrap();
        region.prot = prot;

        #[cfg(target_os = "android")]
        {
            let len = region.size() as usize;
            // SAFETY: FFI into our own mprotect wrapper; `base`/`len` describe a
            // page-aligned region this manager owns and previously mapped.
            let rc = unsafe { ios_emu_native_protect(base, len, prot.0 as u32) };
            if rc != 0 {
                return Err(EmuError::Memory { addr: base, reason: "native mprotect failed" });
            }
            // A region that just became executable may hold freshly-written code
            // (loaded __TEXT, the BRK-filled __stubs page). Publish it to the
            // I-cache now — before any jump into it — or the CPU fetches stale
            // bytes and faults SIGILL. This is the single choke point through
            // which every executable region passes.
            if prot.contains(Protection::EXEC) {
                // SAFETY: FFI cache-flush over the same owned, mapped range.
                unsafe { ios_emu_native_flush_icache(base, len) };
            }
        }
        Ok(())
    }

    /// Allocate `size` bytes from the managed heap (backs `malloc`).
    pub fn heap_alloc(&mut self, size: u64, align: u64) -> EmuResult<GuestAddr> {
        self.heap.alloc(size, align)
    }

    // ---- typed accessors -------------------------------------------------

    /// Read exactly `buf.len()` bytes starting at `addr`.
    pub fn read(&self, addr: GuestAddr, buf: &mut [u8]) -> EmuResult<()> {
        let src = self.slice(addr, buf.len() as u64, Protection::READ)?;
        buf.copy_from_slice(src);
        Ok(())
    }

    /// Write `data` starting at `addr`, honouring write protection.
    pub fn write(&mut self, addr: GuestAddr, data: &[u8]) -> EmuResult<()> {
        let dst = self.slice_mut(addr, data.len() as u64, Protection::WRITE)?;
        dst.copy_from_slice(data);
        Ok(())
    }

    pub fn read_u8(&self, addr: GuestAddr) -> EmuResult<u8> {
        let mut b = [0u8; 1];
        self.read(addr, &mut b)?;
        Ok(b[0])
    }

    pub fn read_u32(&self, addr: GuestAddr) -> EmuResult<u32> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    pub fn read_u64(&self, addr: GuestAddr) -> EmuResult<u64> {
        let mut b = [0u8; 8];
        self.read(addr, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    pub fn write_u32(&mut self, addr: GuestAddr, v: u32) -> EmuResult<()> {
        self.write(addr, &v.to_le_bytes())
    }

    pub fn write_u64(&mut self, addr: GuestAddr, v: u64) -> EmuResult<()> {
        self.write(addr, &v.to_le_bytes())
    }

    /// Read a NUL-terminated C string of at most `max` bytes.
    pub fn read_cstr(&self, addr: GuestAddr, max: usize) -> EmuResult<String> {
        let mut out = Vec::new();
        let mut cur = addr;
        for _ in 0..max {
            let b = self.read_u8(cur)?;
            if b == 0 {
                break;
            }
            out.push(b);
            cur = cur + 1;
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Iterate the mapped regions in ascending address order (for the executor
    /// to mirror, and for crash-dump formatting).
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.regions.values()
    }

    // ---- internals -------------------------------------------------------

    fn region_base(&self, addr: GuestAddr) -> EmuResult<u64> {
        let a = addr.raw();
        if let Some((&base, region)) = self.regions.range(..=a).next_back() {
            if region.contains(addr) {
                return Ok(base);
            }
        }
        Err(EmuError::Memory { addr: a, reason: "unmapped" })
    }

    fn slice(&self, addr: GuestAddr, len: u64, need: Protection) -> EmuResult<&[u8]> {
        let a = addr.raw();
        let (_, region) = self
            .regions
            .range(..=a)
            .next_back()
            .ok_or(EmuError::Memory { addr: a, reason: "unmapped" })?;
        region.subslice(addr, len, need)
    }

    fn slice_mut(&mut self, addr: GuestAddr, len: u64, need: Protection) -> EmuResult<&mut [u8]> {
        let a = addr.raw();
        let (_, region) = self
            .regions
            .range_mut(..=a)
            .next_back()
            .ok_or(EmuError::Memory { addr: a, reason: "unmapped" })?;
        region.subslice_mut(addr, len, need)
    }

    /// Verify `[base, base+size)` overlaps no existing region.
    fn check_free(&self, base: GuestAddr, size: u64) -> EmuResult<()> {
        let start = base.raw();
        let end = start
            .checked_add(size)
            .ok_or(EmuError::Memory { addr: start, reason: "map overflow" })?;
        // The only region that could overlap starts at or before `end`; check
        // the nearest one at or below `start` and the next one above it.
        if let Some((_, r)) = self.regions.range(..end).next_back() {
            if r.base.raw() < end && start < r.end().raw() {
                return Err(EmuError::Memory { addr: start, reason: "map overlaps existing region" });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> GuestMemory {
        GuestMemory::new(GuestAddr(0x1_0000_0000), 0x10000).unwrap()
    }

    #[test]
    fn heap_rw_roundtrip() {
        let mut m = mem();
        let p = m.heap_alloc(64, 8).unwrap();
        m.write_u64(p, 0xdead_beef_cafe_babe).unwrap();
        assert_eq!(m.read_u64(p).unwrap(), 0xdead_beef_cafe_babe);
    }

    #[test]
    fn protection_enforced() {
        let mut m = mem();
        m.map(GuestAddr(0x2_0000_0000), 0x4000, Protection::READ, RegionKind::Anonymous, "ro")
            .unwrap();
        assert!(matches!(
            m.write_u32(GuestAddr(0x2_0000_0000), 1),
            Err(EmuError::Memory { .. })
        ));
    }

    #[test]
    fn overlap_rejected() {
        let mut m = mem();
        m.map(GuestAddr(0x3_0000_0000), 0x8000, Protection::rw(), RegionKind::Anonymous, "a")
            .unwrap();
        assert!(m
            .map(GuestAddr(0x3_0000_4000), 0x8000, Protection::rw(), RegionKind::Anonymous, "b")
            .is_err());
    }

    #[test]
    fn cstr_read() {
        let mut m = mem();
        let p = m.heap_alloc(16, 1).unwrap();
        m.write(p, b"hello\0garbage").unwrap();
        assert_eq!(m.read_cstr(p, 16).unwrap(), "hello");
    }
}
