//! CoreGraphics shims.
//!
//! Most CG entry points pass/return small structs (`CGRect`, `CGPoint`) which,
//! under the AArch64 procedure call standard, travel in `x0..x7`/`d0..d7` or via
//! an `x8` sret pointer. Faithful shims must therefore respect the exact ABI of
//! each function; until each is modelled they are stubbed to zero, which yields
//! `CGRectZero`-like defaults and keeps layout code running.

use ios_emu_core::{CallContext, Dispatch};

use super::Registry;
use crate::objc::ObjcRuntime;

pub fn install(map: &mut Registry) {
    for sym in [
        "CGColorCreate",
        "CGColorRelease",
        "CGContextSetFillColorWithColor",
        "CGContextFillRect",
        "CGContextRef",
        "CGBitmapContextCreate",
        "CGColorSpaceCreateDeviceRGB",
        "CGColorSpaceRelease",
        "CGContextDrawImage",
        "CGImageRelease",
    ] {
        super::register_stub(map, sym);
    }
    map.insert("CGMainDisplayID", cg_main_display_id);
}

/// `CGMainDisplayID()` -> a non-zero display id so callers treat a display as
/// present.
fn cg_main_display_id(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(1);
    Dispatch::Handled
}
