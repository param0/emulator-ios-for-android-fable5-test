//! Darwin (`<sys/errno.h>`) error numbers returned to the guest.
//!
//! These are the *Darwin* values, which differ from Linux in several places
//! (e.g. `EAGAIN`/`EWOULDBLOCK` == 35, not 11). The syscall layer must map host
//! `std::io::Error` kinds to these before returning to the guest.

#![allow(dead_code)]

pub const EPERM: u32 = 1;
pub const ENOENT: u32 = 2;
pub const ESRCH: u32 = 3;
pub const EINTR: u32 = 4;
pub const EIO: u32 = 5;
pub const ENXIO: u32 = 6;
pub const EBADF: u32 = 9;
pub const ECHILD: u32 = 10;
pub const EDEADLK: u32 = 11;
pub const ENOMEM: u32 = 12;
pub const EACCES: u32 = 13;
pub const EFAULT: u32 = 14;
pub const EBUSY: u32 = 16;
pub const EEXIST: u32 = 17;
pub const ENODEV: u32 = 19;
pub const ENOTDIR: u32 = 20;
pub const EISDIR: u32 = 21;
pub const EINVAL: u32 = 22;
pub const ENFILE: u32 = 23;
pub const EMFILE: u32 = 24;
pub const ENOSPC: u32 = 28;
pub const ESPIPE: u32 = 29;
pub const EROFS: u32 = 30;
pub const EAGAIN: u32 = 35;
pub const ENOSYS: u32 = 78;

/// Map a host I/O error to the closest Darwin errno.
pub fn from_io(e: &std::io::Error) -> u32 {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => ENOENT,
        PermissionDenied => EACCES,
        AlreadyExists => EEXIST,
        WouldBlock => EAGAIN,
        InvalidInput => EINVAL,
        BrokenPipe => ESPIPE,
        _ => EIO,
    }
}
