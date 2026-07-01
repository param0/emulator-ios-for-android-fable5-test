//! A minimal bump allocator backing the guest heap.
//!
//! iOS' real allocator is `libmalloc` (magazine/nano). We do not emulate its
//! internals; instead the guest's `malloc`/`free`/`calloc` shims are routed to
//! this arena. It is intentionally simple — bump-only with alignment — which is
//! sufficient for bring-up. A production build swaps in a free-list or hands the
//! guest a real region and lets its own `libmalloc` run against our `mmap`.

use ios_emu_common::{align_up, EmuError, EmuResult, GuestAddr};

pub struct BumpAllocator {
    base: GuestAddr,
    limit: GuestAddr,
    cursor: u64,
}

impl BumpAllocator {
    pub fn new(base: GuestAddr, size: u64) -> BumpAllocator {
        BumpAllocator { base, limit: base + size, cursor: base.raw() }
    }

    /// Allocate `size` bytes aligned to `align` (power of two). Returns the
    /// guest address, or an error if the arena is exhausted.
    pub fn alloc(&mut self, size: u64, align: u64) -> EmuResult<GuestAddr> {
        let align = align.max(1);
        let start = align_up(self.cursor, align);
        let end = start
            .checked_add(size.max(1))
            .ok_or(EmuError::Memory { addr: start, reason: "heap alloc overflow" })?;
        if end > self.limit.raw() {
            return Err(EmuError::Memory { addr: end, reason: "heap exhausted" });
        }
        self.cursor = end;
        Ok(GuestAddr(start))
    }

    /// Bytes handed out so far (for diagnostics).
    pub fn used(&self) -> u64 {
        self.cursor - self.base.raw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_and_exhaustion() {
        let mut a = BumpAllocator::new(GuestAddr(0x1000), 0x100);
        let p = a.alloc(1, 16).unwrap();
        assert_eq!(p.raw() % 16, 0);
        let q = a.alloc(16, 16).unwrap();
        assert!(q.raw() >= p.raw() + 1);
        assert!(a.alloc(0x1000, 1).is_err());
    }
}
