//! `libobjc` entry points: message send and the class/selector C API.

use ios_emu_common::GuestAddr;
use ios_emu_core::{CallContext, Dispatch};

use super::Registry;
use crate::objc::ObjcRuntime;

pub fn install(map: &mut Registry) {
    map.insert("objc_msgSend", objc_msg_send);
    map.insert("objc_msgSendSuper", objc_msg_send); // super-dispatch collapses to msgSend here
    map.insert("objc_msgSendSuper2", objc_msg_send);
    map.insert("objc_getClass", objc_get_class);
    map.insert("objc_lookUpClass", objc_get_class);
    map.insert("objc_getRequiredClass", objc_get_class);
    map.insert("objc_getMetaClass", objc_get_class);
    map.insert("sel_registerName", sel_register_name);
    map.insert("sel_getUid", sel_register_name);
    map.insert("objc_retain", objc_retain);
    map.insert("objc_release", objc_release);
    map.insert("objc_autorelease", objc_retain);
    map.insert("objc_retainAutoreleasedReturnValue", objc_retain);
    map.insert("objc_autoreleaseReturnValue", objc_retain);
    map.insert("objc_alloc", objc_alloc);
    map.insert("objc_alloc_init", objc_alloc);
    map.insert("objc_storeStrong", super::stub_zero);
    map.insert("objc_autoreleasePoolPush", super::stub_zero);
    map.insert("objc_autoreleasePoolPop", super::stub_zero);
}

fn objc_msg_send(rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    rt.msg_send(ctx)
}

fn objc_get_class(rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    let name = ctx.mem.read_cstr(GuestAddr(ctx.arg(0)), 256).unwrap_or_default();
    let addr = rt.get_or_create_class(&name, ctx).map(|a| a.raw()).unwrap_or(0);
    ctx.ret(addr);
    Dispatch::Handled
}

/// `sel_registerName(const char*)` -> `SEL`. Selectors *are* their name pointer
/// in this runtime, so registration is the identity function.
fn sel_register_name(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(ctx.arg(0));
    Dispatch::Handled
}

fn objc_retain(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(ctx.arg(0)); // ARC retain returns its argument
    Dispatch::Handled
}

fn objc_release(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    ctx.ret(0);
    Dispatch::Handled
}

/// `objc_alloc(Class)` -> instance. Modelled as a class-side `alloc` message.
fn objc_alloc(rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    // Reuse msg_send by faking a selector would require memory; instead the
    // runtime exposes allocation directly through get_or_create + msgSend paths.
    // Here we treat arg0 as a class object and allocate against it.
    let class = ctx.arg(0);
    // Route through a synthetic "alloc" send: place selector-less alloc.
    rt.alloc_via(class, ctx);
    Dispatch::Handled
}
