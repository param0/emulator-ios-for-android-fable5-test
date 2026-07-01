//! The lazy Obj-C runtime and `objc_msgSend` dispatcher.

use std::collections::HashMap;

use ios_emu_common::GuestAddr;
use ios_emu_core::{CallContext, Dispatch};

/// Object header written at the start of every emulated instance / class object.
/// The first word is the `isa` (class pointer), matching the real ABI so a guest
/// reading `*(id)obj` gets the class handle back.
const OBJ_HEADER_SIZE: u64 = 16;
const CLASS_MAGIC: u64 = 0x0b1c_0000_0000_0000;

struct ClassInfo {
    name: String,
    /// Index of the superclass in `classes`, if any. Reserved for method
    /// resolution once per-class method tables are populated from `__objc_data`.
    #[allow(dead_code)]
    superclass: Option<usize>,
}

/// Runtime state: the class table plus a map from guest object/class addresses
/// back to their runtime identity, so `objc_msgSend` can tell a class-method
/// send from an instance-method send.
pub struct ObjcRuntime {
    classes: Vec<ClassInfo>,
    /// class name -> guest address of the class object
    class_addr_by_name: HashMap<String, u64>,
    /// class object address -> index into `classes`
    class_index_by_addr: HashMap<u64, usize>,
    /// instance address -> class index
    instance_class: HashMap<u64, usize>,
    /// Retain counts, keyed by instance address (best-effort ARC accounting).
    retain_counts: HashMap<u64, i64>,
}

impl ObjcRuntime {
    pub fn new() -> Self {
        ObjcRuntime {
            classes: Vec::new(),
            class_addr_by_name: HashMap::new(),
            class_index_by_addr: HashMap::new(),
            instance_class: HashMap::new(),
            retain_counts: HashMap::new(),
        }
    }

    /// Register the root classes eagerly so early `objc_getClass("NSObject")`
    /// calls succeed without allocating.
    pub fn preload_root_classes(&mut self) {
        // Registration is lazy/allocating and needs guest memory; the root class
        // objects are actually created on first `objc_getClass`. Here we only
        // reserve their metadata slots so superclass links are stable.
        for name in ["NSObject", "NSProxy"] {
            if !self.classes.iter().any(|c| c.name == name) {
                self.classes.push(ClassInfo { name: name.to_owned(), superclass: None });
            }
        }
    }

    fn class_index(&self, name: &str) -> Option<usize> {
        self.classes.iter().position(|c| c.name == name)
    }

    /// Return the guest address of the class object for `name`, allocating a
    /// class object (and metadata slot) on first use.
    pub fn get_or_create_class(&mut self, name: &str, ctx: &mut CallContext) -> Option<GuestAddr> {
        if let Some(&addr) = self.class_addr_by_name.get(name) {
            return Some(GuestAddr(addr));
        }
        let addr = ctx.mem.heap_alloc(OBJ_HEADER_SIZE, 16).ok()?;
        // A class object's isa is itself (metaclass collapse); tag with a magic.
        ctx.mem.write_u64(addr, CLASS_MAGIC | addr.raw()).ok()?;

        let index = self.class_index(name).unwrap_or_else(|| {
            self.classes.push(ClassInfo { name: name.to_owned(), superclass: self.class_index("NSObject") });
            self.classes.len() - 1
        });
        self.class_addr_by_name.insert(name.to_owned(), addr.raw());
        self.class_index_by_addr.insert(addr.raw(), index);
        Some(addr)
    }

    /// Allocate an instance of `class_addr` and set it as the return value.
    /// Backs the `objc_alloc` fast-path entry point.
    pub fn alloc_via(&mut self, class_addr: u64, ctx: &mut CallContext) {
        let obj = self.alloc_instance(class_addr, ctx).map(|a| a.raw()).unwrap_or(0);
        ctx.ret(obj);
    }

    /// Allocate an instance of the class object at `class_addr`.
    fn alloc_instance(&mut self, class_addr: u64, ctx: &mut CallContext) -> Option<GuestAddr> {
        let &index = self.class_index_by_addr.get(&class_addr)?;
        let obj = ctx.mem.heap_alloc(OBJ_HEADER_SIZE, 16).ok()?;
        ctx.mem.write_u64(obj, class_addr).ok()?; // isa
        self.instance_class.insert(obj.raw(), index);
        self.retain_counts.insert(obj.raw(), 1);
        Some(obj)
    }

    fn class_name(&self, index: usize) -> &str {
        &self.classes[index].name
    }

    /// The core of the runtime: dispatch `objc_msgSend(self, _cmd, ...)`.
    ///
    /// `x0` = receiver, `x1` = selector (a C-string pointer). Returns via `x0`.
    pub fn msg_send(&mut self, ctx: &mut CallContext) -> Dispatch {
        let recv = ctx.arg(0);
        let sel_ptr = GuestAddr(ctx.arg(1));

        // `nil`-receiver messaging returns nil in Obj-C — never a crash.
        if recv == 0 {
            ctx.ret(0);
            return Dispatch::Handled;
        }

        let sel = match ctx.mem.read_cstr(sel_ptr, 256) {
            Ok(s) if !s.is_empty() => s,
            _ => {
                log::warn!("objc_msgSend with unreadable selector @ {sel_ptr}");
                ctx.ret(0);
                return Dispatch::Handled;
            }
        };

        let is_class = self.class_index_by_addr.contains_key(&recv);
        let is_instance = self.instance_class.contains_key(&recv);
        log::trace!(target: "ios_emu::objc", "msgSend {} -> -[{}]",
            if is_class { "class" } else { "inst" }, sel);

        let ret = match sel.as_str() {
            "alloc" | "allocWithZone:" if is_class => {
                self.alloc_instance(recv, ctx).map(|a| a.raw()).unwrap_or(0)
            }
            "new" if is_class => {
                // new == alloc + init
                self.alloc_instance(recv, ctx).map(|a| a.raw()).unwrap_or(0)
            }
            // init family and identity-returning messages return the receiver.
            s if s == "init" || s.starts_with("initWith") => recv,
            "self" | "retain" | "autorelease" => {
                if let Some(c) = self.retain_counts.get_mut(&recv) {
                    if sel == "retain" {
                        *c += 1;
                    }
                }
                recv
            }
            "release" => {
                if let Some(c) = self.retain_counts.get_mut(&recv) {
                    *c -= 1;
                }
                0 // -release returns void
            }
            "retainCount" => self.retain_counts.get(&recv).copied().unwrap_or(1) as u64,
            "class" => {
                if is_instance {
                    let idx = self.instance_class[&recv];
                    self.class_addr_by_name.get(self.class_name(idx)).copied().unwrap_or(0)
                } else {
                    recv // a class's -class is itself
                }
            }
            "respondsToSelector:" | "isKindOfClass:" | "conformsToProtocol:" => 0, // NO
            "hash" => recv, // stable, unique-ish
            other => {
                log::trace!(target: "ios_emu::stub", "STUB objc::-[{other}] -> nil");
                0
            }
        };

        ctx.ret(ret);
        Dispatch::Handled
    }
}

impl Default for ObjcRuntime {
    fn default() -> Self {
        Self::new()
    }
}
