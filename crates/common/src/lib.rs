//! Types shared across every crate in the emulator.
//!
//! This crate is the root of the dependency DAG; it must never depend on any
//! other workspace crate. Keep it free of I/O and platform specifics so the
//! leaf crates stay trivially testable on the host.

#![forbid(unsafe_code)]

pub mod error;
pub mod guest;
pub mod logging;

pub use error::{EmuError, EmuResult};
pub use guest::{GuestAddr, GuestSize, Protection};

/// The virtual base address the main executable is slid to. iOS applies ASLR;
/// we pick a fixed, page-aligned slide so that crash addresses are stable and
/// reproducible during development. Production builds randomise this.
pub const DEFAULT_LOAD_BASE: GuestAddr = GuestAddr(0x0000_0001_0000_0000);

/// AArch64 translation granule used throughout the memory manager.
pub const PAGE_SIZE: u64 = 0x4000; // iOS/arm64 uses 16 KiB pages.

/// Round `value` up to the next multiple of `align` (which must be a power of two).
#[inline]
pub const fn align_up(value: u64, align: u64) -> u64 {
    (value + (align - 1)) & !(align - 1)
}

/// Round `value` down to the previous multiple of `align`.
#[inline]
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_helpers() {
        assert_eq!(align_up(1, PAGE_SIZE), PAGE_SIZE);
        assert_eq!(align_up(PAGE_SIZE, PAGE_SIZE), PAGE_SIZE);
        assert_eq!(align_down(PAGE_SIZE + 1, PAGE_SIZE), PAGE_SIZE);
        assert_eq!(align_down(0x4001, 0x4000), 0x4000);
    }
}
