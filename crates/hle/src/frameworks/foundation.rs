//! Foundation shims. Coverage is intentionally thin — enough to keep launch
//! paths alive and to surface `NSLog` output — with the long tail stubbed.

use ios_emu_common::GuestAddr;
use ios_emu_core::{CallContext, Dispatch};

use super::Registry;
use crate::objc::ObjcRuntime;

/// Offset of the backing C-string pointer inside a `__NSCFConstantString`
/// (`isa`, `flags`, `char *`, `length`).
const CFSTR_CSTR_OFFSET: u64 = 16;

pub fn install(map: &mut Registry) {
    map.insert("NSLog", ns_log);
    map.insert("CFShow", ns_log);
    // Common Foundation C entry points that are safe to no-op / zero.
    for sym in [
        "CFRetain",
        "CFRelease",
        "CFBridgingRetain",
        "CFBridgingRelease",
        "NSSearchPathForDirectoriesInDomains",
        "CFStringCreateWithCString",
        "CFAllocatorGetDefault",
        "NSStringFromClass",
        "objc_setAssociatedObject",
        "objc_getAssociatedObject",
    ] {
        super::register_stub(map, sym);
    }
}

/// Best-effort `NSLog`: if the format argument is a constant `NSString`, pull out
/// its C string and log it. Format specifiers are not expanded (that needs the
/// varargs + `%@` object description machinery); the raw format is logged, which
/// is invaluable for tracing app startup.
fn ns_log(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let nsstring = GuestAddr(ctx.arg(0));
    let message = ctx
        .mem
        .read_u64(nsstring + CFSTR_CSTR_OFFSET)
        .ok()
        .filter(|p| *p != 0)
        .and_then(|cstr| ctx.mem.read_cstr(GuestAddr(cstr), 4096).ok())
        .unwrap_or_else(|| "<unprintable NSString>".to_owned());
    log::info!(target: "ios_emu::NSLog", "{message}");
    ctx.ret(0);
    Dispatch::Handled
}
