//! A single mapped region of guest memory.

use ios_emu_common::{EmuError, EmuResult, GuestAddr, Protection};

/// Provenance of a region, used for diagnostics and for the executor to decide
/// which regions to mirror into an executable host mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// A Mach-O `__TEXT`/`__DATA`/... segment.
    Segment,
    /// The main/secondary thread stack.
    Stack,
    /// The managed malloc heap.
    Heap,
    /// Anonymous `mmap`.
    Anonymous,
    /// Commpage / shared kernel data emulation.
    Commpage,
}

/// A contiguous, page-aligned span of guest memory with a host backing buffer.
pub struct Region {
    pub base: GuestAddr,
    pub prot: Protection,
    pub kind: RegionKind,
    pub name: String,
    data: Vec<u8>,
}

impl Region {
    pub(crate) fn new_zeroed(
        base: GuestAddr,
        size: u64,
        prot: Protection,
        kind: RegionKind,
        name: &str,
    ) -> Region {
        Region { base, prot, kind, name: name.to_owned(), data: vec![0u8; size as usize] }
    }

    pub(crate) fn new_from(
        base: GuestAddr,
        size: u64,
        init: &[u8],
        prot: Protection,
        kind: RegionKind,
        name: &str,
    ) -> EmuResult<Region> {
        if init.len() as u64 > size {
            return Err(EmuError::Memory { addr: base.raw(), reason: "init data exceeds region" });
        }
        let mut data = vec![0u8; size as usize];
        data[..init.len()].copy_from_slice(init);
        Ok(Region { base, prot, kind, name: name.to_owned(), data })
    }

    #[inline]
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }

    #[inline]
    pub fn end(&self) -> GuestAddr {
        self.base + self.size()
    }

    #[inline]
    pub fn contains(&self, addr: GuestAddr) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// Borrow `[addr, addr+len)` for reading, checking bounds and that the
    /// region grants `need`.
    pub fn subslice(&self, addr: GuestAddr, len: u64, need: Protection) -> EmuResult<&[u8]> {
        let (off, end) = self.range(addr, len)?;
        if !self.prot.contains(need) {
            return Err(EmuError::Memory { addr: addr.raw(), reason: "protection violation" });
        }
        Ok(&self.data[off..end])
    }

    /// Mutable counterpart of [`subslice`](Self::subslice).
    pub fn subslice_mut(
        &mut self,
        addr: GuestAddr,
        len: u64,
        need: Protection,
    ) -> EmuResult<&mut [u8]> {
        let (off, end) = self.range(addr, len)?;
        if !self.prot.contains(need) {
            return Err(EmuError::Memory { addr: addr.raw(), reason: "protection violation" });
        }
        Ok(&mut self.data[off..end])
    }

    /// Raw host bytes of the region (used by the executor to publish an
    /// executable mirror). Callers must respect `prot`.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    fn range(&self, addr: GuestAddr, len: u64) -> EmuResult<(usize, usize)> {
        if addr < self.base {
            return Err(EmuError::Memory { addr: addr.raw(), reason: "before region" });
        }
        let off = (addr - self.base) as usize;
        let end = off
            .checked_add(len as usize)
            .ok_or(EmuError::Memory { addr: addr.raw(), reason: "length overflow" })?;
        if end > self.data.len() {
            return Err(EmuError::Memory { addr: addr.raw(), reason: "past region end" });
        }
        Ok((off, end))
    }
}
