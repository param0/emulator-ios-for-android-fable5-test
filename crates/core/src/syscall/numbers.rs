//! BSD syscall numbers from Darwin's `syscalls.master` (the subset we service).
//! The values are ABI-stable across iOS versions, which is what lets one table
//! cover iOS 7 through current releases.

#![allow(dead_code)]

pub const SYS_EXIT: u32 = 1;
pub const SYS_FORK: u32 = 2;
pub const SYS_READ: u32 = 3;
pub const SYS_WRITE: u32 = 4;
pub const SYS_OPEN: u32 = 5;
pub const SYS_CLOSE: u32 = 6;
pub const SYS_GETPID: u32 = 20;
pub const SYS_GETUID: u32 = 24;
pub const SYS_GETEUID: u32 = 25;
pub const SYS_ACCESS: u32 = 33;
pub const SYS_GETEGID: u32 = 43;
pub const SYS_GETGID: u32 = 47;
pub const SYS_MUNMAP: u32 = 73;
pub const SYS_MPROTECT: u32 = 74;
pub const SYS_MADVISE: u32 = 75;
pub const SYS_FCNTL: u32 = 92;
pub const SYS_GETTIMEOFDAY: u32 = 116;
pub const SYS_WRITEV: u32 = 121;
pub const SYS_MMAP: u32 = 197;
pub const SYS_LSEEK: u32 = 199;
pub const SYS_SYSCTL: u32 = 202;
pub const SYS_ISSETUGID: u32 = 327;
pub const SYS_STAT64: u32 = 338;
pub const SYS_FSTAT64: u32 = 339;
pub const SYS_LSTAT64: u32 = 340;
pub const SYS_OPEN_NOCANCEL: u32 = 398;
pub const SYS_CLOSE_NOCANCEL: u32 = 399;
pub const SYS_READ_NOCANCEL: u32 = 396;
pub const SYS_WRITE_NOCANCEL: u32 = 397;
pub const SYS_GETENTROPY: u32 = 500;

// Open flags (`<sys/fcntl.h>`, Darwin values).
pub const O_RDONLY: u64 = 0x0000;
pub const O_WRONLY: u64 = 0x0001;
pub const O_RDWR: u64 = 0x0002;
pub const O_ACCMODE: u64 = 0x0003;
pub const O_APPEND: u64 = 0x0008;
pub const O_CREAT: u64 = 0x0200;
pub const O_TRUNC: u64 = 0x0400;
pub const O_EXCL: u64 = 0x0800;

// mmap prot/flags (`<sys/mman.h>`).
pub const PROT_NONE: u64 = 0x0;
pub const PROT_READ: u64 = 0x1;
pub const PROT_WRITE: u64 = 0x2;
pub const PROT_EXEC: u64 = 0x4;
pub const MAP_ANON: u64 = 0x1000;
pub const MAP_FIXED: u64 = 0x0010;
