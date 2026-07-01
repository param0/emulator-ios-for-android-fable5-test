//! A compact Objective-C runtime sufficient to bring up real apps.
//!
//! We do not parse the binary's `__objc_*` metadata sections into a faithful
//! class hierarchy; instead classes are materialised lazily. The guest's
//! selectors are C strings (the `SEL`s dyld bound point straight at the
//! `__objc_methname` strings), so [`ObjcRuntime::msg_send`] can read the
//! selector name from `x1` and dispatch on it. This handles the overwhelmingly
//! common idioms — `[[Class alloc] init]`, `retain`/`release`/`autorelease`,
//! `class`, `respondsToSelector:` — and logs anything else as a stub returning
//! `nil`, which keeps apps running instead of crashing on an unknown message.

mod runtime;

pub use runtime::ObjcRuntime;
