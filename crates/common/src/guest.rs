//! Newtypes for guest (iOS-side) addresses and sizes.
//!
//! Wrapping raw `u64`s in distinct types prevents the classic emulator bug of
//! accidentally mixing host pointers with guest virtual addresses, or byte
//! counts with addresses, in arithmetic.

use core::fmt;
use core::ops::{Add, Sub};

/// A 64-bit virtual address in the emulated iOS process' address space.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GuestAddr(pub u64);

/// A size/length in bytes within the guest address space.
pub type GuestSize = u64;

impl GuestAddr {
    pub const NULL: GuestAddr = GuestAddr(0);

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Align this address down to `align` (a power of two).
    #[inline]
    pub const fn align_down(self, align: u64) -> GuestAddr {
        GuestAddr(self.0 & !(align - 1))
    }

    /// Align this address up to `align` (a power of two).
    #[inline]
    pub const fn align_up(self, align: u64) -> GuestAddr {
        GuestAddr((self.0 + (align - 1)) & !(align - 1))
    }

    /// Checked byte offset; returns `None` on overflow.
    #[inline]
    pub fn checked_add(self, off: u64) -> Option<GuestAddr> {
        self.0.checked_add(off).map(GuestAddr)
    }
}

impl Add<u64> for GuestAddr {
    type Output = GuestAddr;
    #[inline]
    fn add(self, rhs: u64) -> GuestAddr {
        GuestAddr(self.0.wrapping_add(rhs))
    }
}

impl Sub<GuestAddr> for GuestAddr {
    type Output = u64;
    #[inline]
    fn sub(self, rhs: GuestAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl fmt::Debug for GuestAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GuestAddr({:#018x})", self.0)
    }
}

impl fmt::Display for GuestAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

/// Page-protection flags for a guest memory region. Mirrors Mach `vm_prot_t`
/// so segment `initprot`/`maxprot` values map straight through.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Protection(pub u8);

impl Protection {
    pub const NONE: Protection = Protection(0);
    pub const READ: Protection = Protection(1 << 0);
    pub const WRITE: Protection = Protection(1 << 1);
    pub const EXEC: Protection = Protection(1 << 2);

    #[inline]
    pub const fn rw() -> Protection {
        Protection(Self::READ.0 | Self::WRITE.0)
    }

    #[inline]
    pub const fn rx() -> Protection {
        Protection(Self::READ.0 | Self::EXEC.0)
    }

    #[inline]
    pub const fn contains(self, other: Protection) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Build from a Mach `vm_prot_t` (low three bits are R/W/X).
    #[inline]
    pub const fn from_vm_prot(vm_prot: u32) -> Protection {
        Protection((vm_prot & 0b111) as u8)
    }
}

impl fmt::Debug for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}",
            if self.contains(Self::READ) { 'r' } else { '-' },
            if self.contains(Self::WRITE) { 'w' } else { '-' },
            if self.contains(Self::EXEC) { 'x' } else { '-' },
        )
    }
}
