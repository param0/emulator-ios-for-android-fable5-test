//! BSD/Unix syscall handlers.
//!
//! Each handler pulls arguments from `x0..x5`, performs the effect against guest
//! memory / the sandboxed host filesystem, and returns `Ok(value)` or
//! `Err(errno)`. [`handle`] returns `None` for numbers we do not implement so
//! the dispatcher can log-and-stub them uniformly.

use std::io::{Read, Seek, SeekFrom, Write};

use ios_emu_common::{GuestAddr, Protection};

use super::errno::*;
use super::numbers::*;
use super::SyscallCtx;
use crate::fd::FdObject;

/// Dispatch a BSD syscall by number. `None` => not implemented.
pub fn handle(number: u32, ctx: &mut SyscallCtx) -> Option<Result<u64, u32>> {
    let r = match number {
        SYS_EXIT => sys_exit(ctx),
        SYS_READ | SYS_READ_NOCANCEL => sys_read(ctx),
        SYS_WRITE | SYS_WRITE_NOCANCEL => sys_write(ctx),
        SYS_WRITEV => sys_writev(ctx),
        SYS_OPEN | SYS_OPEN_NOCANCEL => sys_open(ctx),
        SYS_CLOSE | SYS_CLOSE_NOCANCEL => sys_close(ctx),
        SYS_LSEEK => sys_lseek(ctx),
        SYS_ACCESS => sys_access(ctx),
        SYS_MMAP => sys_mmap(ctx),
        SYS_MUNMAP => Ok(0), // regions are reclaimed at teardown; treat as no-op
        SYS_MPROTECT => sys_mprotect(ctx),
        SYS_MADVISE => Ok(0),
        SYS_GETPID => Ok(1),
        SYS_GETUID | SYS_GETEUID => Ok(501), // mobile user
        SYS_GETGID | SYS_GETEGID => Ok(501),
        SYS_ISSETUGID => Ok(0),
        SYS_GETTIMEOFDAY => sys_gettimeofday(ctx),
        SYS_GETENTROPY => sys_getentropy(ctx),
        SYS_FCNTL => Ok(0), // pretend success for F_SETFD/F_GETFL probes
        _ => return None,
    };
    Some(r)
}

fn sys_exit(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    *ctx.exit = Some(ctx.cpu.arg(0) as i32);
    Ok(0)
}

fn sys_read(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let (fd, buf, count) = (ctx.cpu.arg(0) as i32, GuestAddr(ctx.cpu.arg(1)), ctx.cpu.arg(2) as usize);
    let mut tmp = vec![0u8; count];
    let n = match ctx.fds.get_mut(fd) {
        Some(FdObject::File(f)) => f.read(&mut tmp).map_err(|e| from_io(&e))?,
        Some(FdObject::Stdin) => 0, // no interactive stdin
        Some(_) => return Err(EINVAL),
        None => return Err(EBADF),
    };
    ctx.mem.write(buf, &tmp[..n]).map_err(|_| EFAULT)?;
    Ok(n as u64)
}

fn sys_write(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let (fd, buf, count) = (ctx.cpu.arg(0) as i32, GuestAddr(ctx.cpu.arg(1)), ctx.cpu.arg(2) as usize);
    let mut tmp = vec![0u8; count];
    ctx.mem.read(buf, &mut tmp).map_err(|_| EFAULT)?;
    write_bytes(ctx, fd, &tmp)
}

fn sys_writev(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let (fd, iov, iovcnt) =
        (ctx.cpu.arg(0) as i32, GuestAddr(ctx.cpu.arg(1)), ctx.cpu.arg(2) as usize);
    let mut total = 0u64;
    for i in 0..iovcnt {
        let base_addr = iov + (i as u64 * 16);
        let base = ctx.mem.read_u64(base_addr).map_err(|_| EFAULT)?;
        let len = ctx.mem.read_u64(base_addr + 8).map_err(|_| EFAULT)? as usize;
        let mut tmp = vec![0u8; len];
        ctx.mem.read(GuestAddr(base), &mut tmp).map_err(|_| EFAULT)?;
        total += write_bytes(ctx, fd, &tmp)?;
    }
    Ok(total)
}

/// Shared write path for `write`/`writev`. Standard streams are surfaced through
/// the logging backend (logcat on device); files hit the sandbox.
fn write_bytes(ctx: &mut SyscallCtx, fd: i32, data: &[u8]) -> Result<u64, u32> {
    match ctx.fds.get_mut(fd) {
        Some(FdObject::Stdout) | Some(FdObject::Stderr) => {
            log::info!(target: "ios_emu::guest", "{}", String::from_utf8_lossy(data).trim_end());
            Ok(data.len() as u64)
        }
        Some(FdObject::File(f)) => {
            f.write_all(data).map_err(|e| from_io(&e))?;
            Ok(data.len() as u64)
        }
        Some(_) => Err(EINVAL),
        None => Err(EBADF),
    }
}

fn sys_open(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let path_ptr = GuestAddr(ctx.cpu.arg(0));
    let flags = ctx.cpu.arg(1);
    let ios_path = ctx.mem.read_cstr(path_ptr, 1024).map_err(|_| EFAULT)?;
    let (host, read_only) = ctx.vfs.translate(&ios_path).map_err(|_| ENOENT)?;

    let wants_write = (flags & O_ACCMODE) != O_RDONLY;
    if read_only && wants_write {
        return Err(EROFS);
    }

    let mut opts = std::fs::OpenOptions::new();
    match flags & O_ACCMODE {
        O_RDONLY => {
            opts.read(true);
        }
        O_WRONLY => {
            opts.write(true);
        }
        _ => {
            opts.read(true).write(true);
        }
    }
    opts.create(flags & O_CREAT != 0)
        .truncate(flags & O_TRUNC != 0)
        .append(flags & O_APPEND != 0)
        .create_new(flags & O_EXCL != 0 && flags & O_CREAT != 0);

    let file = opts.open(&host).map_err(|e| from_io(&e))?;
    Ok(ctx.fds.insert(FdObject::File(file)) as u64)
}

fn sys_close(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let fd = ctx.cpu.arg(0) as i32;
    if ctx.fds.close(fd) {
        Ok(0)
    } else {
        Err(EBADF)
    }
}

fn sys_lseek(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let (fd, offset, whence) = (ctx.cpu.arg(0) as i32, ctx.cpu.arg(1) as i64, ctx.cpu.arg(2));
    match ctx.fds.get_mut(fd) {
        Some(FdObject::File(f)) => {
            let pos = match whence {
                0 => SeekFrom::Start(offset as u64), // SEEK_SET
                1 => SeekFrom::Current(offset),       // SEEK_CUR
                2 => SeekFrom::End(offset),           // SEEK_END
                _ => return Err(EINVAL),
            };
            f.seek(pos).map_err(|e| from_io(&e))
        }
        Some(_) => Err(ESPIPE),
        None => Err(EBADF),
    }
}

fn sys_access(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let ios_path = ctx.mem.read_cstr(GuestAddr(ctx.cpu.arg(0)), 1024).map_err(|_| EFAULT)?;
    let (host, _) = ctx.vfs.translate(&ios_path).map_err(|_| ENOENT)?;
    if host.exists() {
        Ok(0)
    } else {
        Err(ENOENT)
    }
}

fn sys_mmap(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let len = ctx.cpu.arg(1);
    let prot = ctx.cpu.arg(2);
    let flags = ctx.cpu.arg(3);
    // We only honour anonymous mappings from the guest; file-backed mappings are
    // materialised by the loader, and the guest's `libmalloc` uses MAP_ANON.
    if flags & MAP_ANON == 0 {
        log::warn!("mmap of file-backed mapping is not supported; returning ENOSYS");
        return Err(ENOSYS);
    }
    let region_prot = Protection((prot & 0b111) as u8);
    match ctx.mem.mmap_anon(len, region_prot) {
        Ok(addr) => Ok(addr.raw()),
        Err(_) => Err(ENOMEM),
    }
}

fn sys_mprotect(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let addr = GuestAddr(ctx.cpu.arg(0));
    let prot = ctx.cpu.arg(2);
    ctx.mem
        .protect(addr, Protection((prot & 0b111) as u8))
        .map(|_| 0)
        .map_err(|_| EINVAL)
}

fn sys_gettimeofday(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let tv = GuestAddr(ctx.cpu.arg(0));
    if tv.is_null() {
        return Ok(0);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ctx.mem.write_u64(tv, now.as_secs()).map_err(|_| EFAULT)?;
    ctx.mem.write_u32(tv + 8, now.subsec_micros()).map_err(|_| EFAULT)?;
    Ok(0)
}

fn sys_getentropy(ctx: &mut SyscallCtx) -> Result<u64, u32> {
    let buf = GuestAddr(ctx.cpu.arg(0));
    let len = ctx.cpu.arg(1) as usize;
    if len > 256 {
        return Err(EINVAL); // getentropy caps at 256 bytes
    }
    // A xorshift stream seeded from a monotonic nanosecond clock. Adequate for
    // ASLR cookies / stack canaries during bring-up; a device build wires this
    // to the host `getrandom`.
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    ctx.mem.write(buf, &bytes[..len]).map_err(|_| EFAULT)?;
    Ok(0)
}
