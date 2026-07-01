//! On-disk constants from `<mach-o/loader.h>`, `<mach-o/fat.h>` and
//! `<mach/machine.h>`. Only the subset the loader consumes is defined.

// ---- magics ---------------------------------------------------------------

pub const MH_MAGIC_64: u32 = 0xfeed_facf; // 64-bit, host-endian
pub const MH_CIGAM_64: u32 = 0xcffa_edfe; // 64-bit, byte-swapped
pub const FAT_MAGIC: u32 = 0xcafe_babe; // universal, big-endian fields
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;

// ---- cpu types ------------------------------------------------------------

pub const CPU_ARCH_ABI64: u32 = 0x0100_0000;
pub const CPU_TYPE_ARM: u32 = 12;
pub const CPU_TYPE_ARM64: u32 = CPU_TYPE_ARM | CPU_ARCH_ABI64;

pub const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
pub const CPU_SUBTYPE_ARM64E: u32 = 2;

// ---- mach header filetypes ------------------------------------------------

pub const MH_EXECUTE: u32 = 0x2;
pub const MH_DYLIB: u32 = 0x6;
pub const MH_BUNDLE: u32 = 0x8;

// ---- mach header flags ----------------------------------------------------

pub const MH_PIE: u32 = 0x0020_0000; // position-independent executable

// ---- load commands --------------------------------------------------------

/// `LC_REQ_DYLD` marks a command dyld must understand or refuse to load.
pub const LC_REQ_DYLD: u32 = 0x8000_0000;

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_SYMTAB: u32 = 0x02;
pub const LC_DYSYMTAB: u32 = 0x0b;
pub const LC_LOAD_DYLIB: u32 = 0x0c;
pub const LC_ID_DYLIB: u32 = 0x0d;
pub const LC_LOAD_DYLINKER: u32 = 0x0e;
pub const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
pub const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
pub const LC_UUID: u32 = 0x1b;
pub const LC_UNIXTHREAD: u32 = 0x05;
pub const LC_ENCRYPTION_INFO_64: u32 = 0x2c;
pub const LC_DYLD_INFO: u32 = 0x22;
pub const LC_DYLD_INFO_ONLY: u32 = 0x22 | LC_REQ_DYLD;
pub const LC_FUNCTION_STARTS: u32 = 0x26;
pub const LC_MAIN: u32 = 0x28 | LC_REQ_DYLD;
pub const LC_SOURCE_VERSION: u32 = 0x2a;
pub const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;

// ---- ARM64 thread-state flavour (for LC_UNIXTHREAD) -----------------------

/// `ARM_THREAD_STATE64` flavour id.
pub const ARM_THREAD_STATE64: u32 = 6;
/// Count (in u32 words) of `arm_thread_state64_t`.
pub const ARM_THREAD_STATE64_COUNT: u32 = 68;

// ---- section type mask ----------------------------------------------------

pub const SECTION_TYPE: u32 = 0x0000_00ff;
pub const S_ZEROFILL: u32 = 0x1; // BSS-style, no file backing
pub const S_MOD_INIT_FUNC_POINTERS: u32 = 0x9; // C++/ObjC static initialisers
