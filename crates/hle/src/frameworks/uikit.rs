//! UIKit shims. The pivotal entry point is `UIApplicationMain`, which on real
//! iOS never returns — it installs the app delegate and enters the run loop.
//! Here it logs, signals that the app "started", and returns 0 so a headless
//! bring-up run terminates cleanly instead of spinning.

use ios_emu_core::{CallContext, Dispatch};

use super::Registry;
use crate::objc::ObjcRuntime;

pub fn install(map: &mut Registry) {
    map.insert("UIApplicationMain", ui_application_main);
    for sym in [
        "UIGraphicsGetCurrentContext",
        "UIGraphicsBeginImageContext",
        "UIGraphicsEndImageContext",
        "UIImagePNGRepresentation",
        "UIScreenMainScreen",
    ] {
        super::register_stub(map, sym);
    }
}

/// `int UIApplicationMain(int argc, char *argv[], NSString *principal,
/// NSString *delegate)`.
fn ui_application_main(_rt: &mut ObjcRuntime, ctx: &mut CallContext) -> Dispatch {
    log::info!(
        target: "ios_emu::UIKit",
        "UIApplicationMain(argc={}, principal={:#x}, delegate={:#x}) — entering emulated run loop",
        ctx.arg(0),
        ctx.arg(2),
        ctx.arg(3),
    );
    // A full implementation would: instantiate the delegate class, send
    // -application:didFinishLaunchingWithOptions:, then pump a CFRunLoop wired to
    // the Android frontend's event/vsync source. For headless bring-up we return.
    ctx.ret(0);
    Dispatch::Handled
}
