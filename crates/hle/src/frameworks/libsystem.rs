//! `libSystem` C runtime: the allocator and the `mem*`/`str*` primitives that
//! nearly every binary imports. These operate directly on guest memory through
//! the [`CallContext`], so they compose with the guest's own data structures.

use ios_emu_common::GuestAddr;
use ios_emu_core::{CallContext, Dispatch};

use super::Registry;
use crate::objc::ObjcRuntime;

pub fn install(map: &mut Registry) {
    map.insert("malloc", malloc);
    map.insert("calloc", calloc);
    map.insert("realloc", realloc);
    map.insert("free", free);
    map.insert("memcpy", memcpy);
    map.insert("memmove", memcpy);
    map.insert("memset", memset);
    map.insert("bzero", bzero);
    map.insert("strlen", strlen);
    map.insert("strcmp", strcmp);
    map.insert("memcmp", memcmp);
    // Threading / locking probes that are safe to no-op in a single-threaded core.
    map.insert("pthread_mutex_lock", super::stub_zero);
    map.insert("pthread_mutex_unlock", super::stub_zero);
    map.insert("pthread_mutex_init", super::stub_zero);
    map.insert("pthread_once", pthread_once);
    map.insert("dispatch_once", pthread_once);
    map.insert("__stack_chk_fail", super::stub_zero);
}

fn malloc(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let size = ctx.arg(0);
    let addr = ctx.mem.heap_alloc(size, 16).map(|a| a.raw()).unwrap_or(0);
    ctx.ret(addr);
    Dispatch::Handled
}

fn calloc(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let total = ctx.arg(0).saturating_mul(ctx.arg(1));
    match ctx.mem.heap_alloc(total, 16) {
        Ok(addr) => {
            // Heap regions are zero-initialised on map, so no explicit clear.
            ctx.ret(addr.raw());
        }
        Err(_) => ctx.ret(0),
    }
    Dispatch::Handled
}

/// `realloc` without a size table: allocate fresh and copy a bounded prefix.
/// Correct enough for growth-only usage during bring-up.
fn realloc(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let old = GuestAddr(ctx.arg(0));
    let size = ctx.arg(1);
    let new = match ctx.mem.heap_alloc(size, 16) {
        Ok(a) => a,
        Err(_) => {
            ctx.ret(0);
            return Dispatch::Handled;
        }
    };
    if !old.is_null() {
        // Copy up to `size` bytes; reads past the old block's real end land in
        // the same zero-filled heap region and are harmless.
        let mut tmp = vec![0u8; size as usize];
        if ctx.mem.read(old, &mut tmp).is_ok() {
            let _ = ctx.mem.write(new, &tmp);
        }
    }
    ctx.ret(new.raw());
    Dispatch::Handled
}

fn free(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    // Bump allocator never reclaims; free is a no-op.
    ctx.ret(0);
    Dispatch::Handled
}

fn memcpy(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let (dst, src, n) = (GuestAddr(ctx.arg(0)), GuestAddr(ctx.arg(1)), ctx.arg(2) as usize);
    let mut tmp = vec![0u8; n];
    if ctx.mem.read(src, &mut tmp).is_ok() {
        let _ = ctx.mem.write(dst, &tmp);
    }
    ctx.ret(dst.raw()); // memcpy/memmove return dst
    Dispatch::Handled
}

fn memset(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let (dst, byte, n) = (GuestAddr(ctx.arg(0)), ctx.arg(1) as u8, ctx.arg(2) as usize);
    let _ = ctx.mem.write(dst, &vec![byte; n]);
    ctx.ret(dst.raw());
    Dispatch::Handled
}

fn bzero(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let (dst, n) = (GuestAddr(ctx.arg(0)), ctx.arg(1) as usize);
    let _ = ctx.mem.write(dst, &vec![0u8; n]);
    ctx.ret(0);
    Dispatch::Handled
}

fn strlen(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let s = ctx.mem.read_cstr(GuestAddr(ctx.arg(0)), 1 << 20).unwrap_or_default();
    ctx.ret(s.len() as u64);
    Dispatch::Handled
}

fn strcmp(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let a = ctx.mem.read_cstr(GuestAddr(ctx.arg(0)), 1 << 16).unwrap_or_default();
    let b = ctx.mem.read_cstr(GuestAddr(ctx.arg(1)), 1 << 16).unwrap_or_default();
    let r = match a.as_bytes().cmp(b.as_bytes()) {
        std::cmp::Ordering::Less => -1i64,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    ctx.ret(r as u64);
    Dispatch::Handled
}

fn memcmp(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let (a, b, n) = (GuestAddr(ctx.arg(0)), GuestAddr(ctx.arg(1)), ctx.arg(2) as usize);
    let mut ba = vec![0u8; n];
    let mut bb = vec![0u8; n];
    let _ = ctx.mem.read(a, &mut ba);
    let _ = ctx.mem.read(b, &mut bb);
    let r = match ba.cmp(&bb) {
        std::cmp::Ordering::Less => -1i64,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    ctx.ret(r as u64);
    Dispatch::Handled
}

/// `pthread_once`/`dispatch_once`: invoke-once guards. Our core runs the init
/// block by convention elsewhere; here we simply report "already done" so the
/// guest proceeds. (A full implementation would call the block via the executor.)
fn pthread_once(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(0);
    Dispatch::Handled
}
