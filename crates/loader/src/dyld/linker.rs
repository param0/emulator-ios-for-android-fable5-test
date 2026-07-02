//! Symbol resolution to HLE trampolines.
//!
//! The [`SymbolResolver`] trait is the seam between the loader and the rest of
//! the emulator: given an imported symbol name, hand back the guest address the
//! GOT slot should point at. The default [`StubResolver`] hands out consecutive
//! addresses from a reserved *trampoline page* and remembers the mapping, so the
//! executor can turn a branch-into-the-stub-page back into "the app called
//! `objc_msgSend`" / "the app called `open`".

use std::collections::HashMap;

use ios_emu_common::GuestAddr;

/// Resolves an imported symbol to the guest address a pointer should be bound
/// to. `None` means "leave unbound" (only valid for weak imports, which bind to
/// NULL).
pub trait SymbolResolver {
    fn resolve(&mut self, symbol: &str, lib_ordinal: i64, weak: bool) -> Option<GuestAddr>;
}

/// The reverse map the executor consults: trampoline address -> symbol name.
pub type TrampolineTable = HashMap<u64, String>;

/// Assigns a unique, stable trampoline address to every distinct imported
/// symbol. Trampolines occupy `[base, base + count*stride)`; the region is
/// mapped read/execute by `core` and filled with `BRK #imm` / `SVC` sleds so an
/// accidental fall-through faults loudly instead of running wild.
pub struct StubResolver {
    base: u64,
    stride: u64,
    next: u64,
    /// symbol -> assigned trampoline
    forward: HashMap<String, GuestAddr>,
    /// trampoline -> symbol (handed to the executor)
    reverse: TrampolineTable,
}

impl StubResolver {
    /// `base` must be page-aligned and outside every image segment. `stride` is
    /// the bytes reserved per trampoline (16 is plenty for a `BRK`+padding).
    pub fn new(base: GuestAddr, stride: u64) -> Self {
        StubResolver {
            base: base.raw(),
            stride,
            next: base.raw(),
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Total bytes of trampoline space handed out so far (for mapping).
    pub fn used(&self) -> u64 {
        self.next - self.base
    }

    /// Consume the resolver and return the address->symbol table for the executor.
    pub fn into_table(self) -> TrampolineTable {
        self.reverse
    }

    /// Borrow the reverse table without consuming.
    pub fn table(&self) -> &TrampolineTable {
        &self.reverse
    }

    /// The forward symbol -> trampoline-address map, for callers that must
    /// resolve a symbol *name* to its stub (e.g. `dyld_stub_binder` patching a
    /// lazy pointer).
    pub fn forward(&self) -> HashMap<String, u64> {
        self.forward.iter().map(|(k, v)| (k.clone(), v.raw())).collect()
    }
}

impl SymbolResolver for StubResolver {
    fn resolve(&mut self, symbol: &str, _lib_ordinal: i64, _weak: bool) -> Option<GuestAddr> {
        if let Some(&addr) = self.forward.get(symbol) {
            return Some(addr);
        }
        let addr = GuestAddr(self.next);
        self.next += self.stride;
        self.forward.insert(symbol.to_owned(), addr);
        self.reverse.insert(addr.raw(), symbol.to_owned());
        Some(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_unique() {
        let mut r = StubResolver::new(GuestAddr(0x4_0000_0000), 16);
        let a = r.resolve("objc_msgSend", 1, false).unwrap();
        let b = r.resolve("open", 2, false).unwrap();
        let a2 = r.resolve("objc_msgSend", 1, false).unwrap();
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(r.table().get(&a.raw()).map(String::as_str), Some("objc_msgSend"));
    }
}
