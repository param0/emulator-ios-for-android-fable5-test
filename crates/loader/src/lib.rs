//! 64-bit Mach-O binary loader and `dyld` scaffolding.
//!
//! The loader is deliberately *pure*: [`MachOImage::parse`] turns a byte slice
//! into a fully-described [`MachOImage`] (segments, entry point, dylib imports,
//! symbol tables, encryption info) without touching guest memory. The `core`
//! crate consumes that description and maps segments through
//! `ios_emu_memory::GuestMemory`, which keeps parsing testable on the host and
//! keeps the mapping policy (slide, ASLR, protections) in one place.
//!
//! Supported containers:
//!   * thin `MH_MAGIC_64` Mach-O (little-endian arm64/arm64e), and
//!   * `FAT_MAGIC`/`FAT_MAGIC_64` universal binaries, from which the arm64
//!     slice is selected.

#![forbid(unsafe_code)]

pub mod dyld;
pub mod macho;

pub use macho::image::{Dylib, MachOImage, Section, Segment};
pub use macho::reader::Reader;
