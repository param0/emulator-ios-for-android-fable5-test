//! Guest file-descriptor table.
//!
//! Guest fds are virtualised: descriptors 0/1/2 are wired to the host
//! stdout/stderr for logging, and everything else maps to a host [`File`]
//! obtained through the sandbox. Keeping our own table (rather than handing the
//! guest raw host fds) means the guest can never reference a host resource we
//! did not deliberately expose.

use std::collections::HashMap;
use std::fs::File;

/// The kind of object a guest fd refers to.
pub enum FdObject {
    /// Standard streams, routed to the host process' stdio / logcat.
    Stdout,
    Stderr,
    Stdin,
    /// A regular file opened inside the sandbox.
    File(File),
    /// A directory handle (for `getdirentries`).
    Dir(std::fs::ReadDir),
}

/// Per-process descriptor table with lowest-available-fd allocation, matching
/// POSIX semantics the guest relies on.
pub struct FdTable {
    slots: HashMap<i32, FdObject>,
    next: i32,
}

impl Default for FdTable {
    fn default() -> Self {
        let mut slots = HashMap::new();
        slots.insert(0, FdObject::Stdin);
        slots.insert(1, FdObject::Stdout);
        slots.insert(2, FdObject::Stderr);
        FdTable { slots, next: 3 }
    }
}

impl FdTable {
    /// Insert `obj` at the lowest free descriptor >= 3 and return it.
    pub fn insert(&mut self, obj: FdObject) -> i32 {
        // Reuse the lowest freed slot to mimic POSIX allocation.
        let mut fd = 3;
        while self.slots.contains_key(&fd) {
            fd += 1;
        }
        self.next = self.next.max(fd + 1);
        self.slots.insert(fd, obj);
        fd
    }

    pub fn get_mut(&mut self, fd: i32) -> Option<&mut FdObject> {
        self.slots.get_mut(&fd)
    }

    pub fn contains(&self, fd: i32) -> bool {
        self.slots.contains_key(&fd)
    }

    /// Close a descriptor. Returns `true` if it existed. Standard streams cannot
    /// be closed (the guest closing stdout is silently ignored).
    pub fn close(&mut self, fd: i32) -> bool {
        if (0..=2).contains(&fd) {
            return true;
        }
        self.slots.remove(&fd).is_some()
    }
}
