# Architecture

This document explains how the emulator is put together and why. It assumes
familiarity with Mach-O, the AArch64 procedure call standard, Darwin syscalls,
and the Objective-C runtime.

## 1. Execution strategy: native-with-traps, not interpretation

The single most important design decision is that the host is **also AArch64**.
Interpreting or JIT-recompiling every instruction would throw away 1–2 orders of
magnitude of performance for no benefit on the vast majority of instructions,
which behave identically on the host. So the guest's real instruction stream is
executed on the host CPU. Only two categories of instruction must be diverted:

1. **`svc #0x80` (Darwin syscall).** Executed directly, it would trap into the
   *Linux* kernel carrying a *Darwin* syscall number — semantic nonsense. At load
   time the native backend rewrites every `svc #0x80` in executable segments to a
   `brk` carrying a private immediate (`native/src/jit_exec.c::prepare_text`).
2. **Imported library calls.** These are indirected through a GOT slot that dyld
   would fill with the real function address. We instead bind every import to a
   unique address in a reserved **trampoline page** (`crates/loader/src/dyld`),
   which is filled with `brk`. A guest `BL` into it traps.

Both produce a `SIGTRAP`, caught by a handler that snapshots the CPU state into
the shared [`CpuContext`](../crates/core/src/cpu.rs), classifies the trap, and
`siglongjmp`s back into `ios_emu_native_resume`. That returns an
[`ExecEvent`](../crates/core/src/cpu.rs) to the Rust
[`Machine::run`](../crates/core/src/machine.rs) loop, which services it and
resumes. The control policy is therefore entirely in safe Rust; the C is only the
register-install/trap-capture mechanism.

### The Rust⇄C ABI

`native/include/ios_emu_jit.h` defines the contract. `guest_cpu_context` is a
byte-for-byte mirror of the `#[repr(C)]` Rust `CpuContext` (asserted with
`_Static_assert`s on the field offsets, which the inline assembly hard-codes).
`ios_emu_native_resume` installs the register file — using the platform-reserved
`x18` as the branch scratch, since both Android and iOS reserve it — and branches
to `pc`. The four out-params carry the event payload (syscall immediate,
trampoline address, or exit code).

## 2. Address-space model

`ios_emu_memory::GuestMemory` is the authority on layout and contents: a
`BTreeMap` of page-aligned, host-backed [`Region`](../crates/memory/src/region.rs)s
with per-region protection enforced on every typed access. The default layout
(`MachineConfig`) keeps everything in disjoint ranges of the low 8 GiB:

| Range | Purpose |
|---|---|
| `0x1_0000_0000+` | Mach-O image segments (mapped at preferred vmaddr) |
| `0x1_2000_0000+` | managed heap (`malloc` arena) |
| `0x1_C000_0000` (top) | main thread stack (grows down) |
| `0x2_0000_0000` (top) | anonymous `mmap` arena (grows down) |
| `0x5_0000_0000+` | import trampoline page |

On device the native `vm_bridge.c` mirrors each region into an actual `mmap` at
the same guest address so native loads/stores resolve; `GuestMemory` still owns
the canonical bytes.

### Slide & binding policy

We map at the image's **preferred** address (slide 0). This keeps every absolute
pointer the linker baked in — rebased data pointers, Obj-C metadata, initializer
arrays — valid without replaying the `LC_DYLD_INFO` rebase stream. Binds *are*
processed: `crates/core/src/loader.rs` walks the (eager + lazy) bind opcode
streams, resolves each symbol to a trampoline via the `StubResolver`, and writes
the pointer into the GOT slot. Segments are mapped writable during load and
downgraded to their real protections afterward, exactly as dyld does. Randomising
the slide (and thus processing rebases) is the documented next step; the seam is
`LayoutConfig::slide`.

## 3. Syscall translation

`crates/core/src/syscall/` decodes the `x16` selector into a
`(class, number)` pair (`decode`): negative → Mach trap, high-byte-tagged →
explicit class, otherwise a BSD/Unix number. BSD handlers
(`unix.rs`) implement the file, memory, time, and process-identity calls against
the sandbox and guest memory, returning results with the Darwin **cerror**
convention (carry flag = error, `x0` = errno). Mach traps (`mach.rs`) return
plausible constants, with `mach_vm_allocate` wired to the memory manager.
Anything unimplemented is logged (`ios_emu::stub`) and given `ENOSYS`/0 — never a
crash. errno values are the *Darwin* set (`errno.rs`), mapped from host
`io::Error` kinds.

## 4. High-Level Emulation

`ios_emu_hle::HleEnvironment` implements the core's `HostEnvironment` trait. It
owns a lazy [`ObjcRuntime`](../crates/hle/src/objc/runtime.rs) and a flat
`HashMap<&'static str, ShimFn>` registry. When the guest calls an import, the
core passes the symbol name; the registry either services it or reports
`Unhandled` (→ generic stub).

* **`objc_msgSend`** reads the receiver (`x0`) and selector (`x1`). Because the
  `SEL`s dyld bound point straight at the `__objc_methname` C strings, the
  selector name is read directly from guest memory. The runtime materialises
  classes on demand, tracks instances and retain counts, and services the
  ubiquitous idioms (`alloc`/`init`/`new`, `retain`/`release`/`autorelease`,
  `class`), logging unknown selectors as stubs returning `nil`.
* **`libSystem`** implements the allocator and the `mem*`/`str*` primitives
  directly against guest memory, so they interoperate with the guest's own data.
* **Foundation/UIKit/CoreGraphics/GLES** provide the load-bearing entry points
  (`NSLog`, `UIApplicationMain`) and register the long tail as explicit stubs.
  GLES is the seam where each `gl*` call marshals to Android's native driver.

Adding coverage never touches the core loop — it is a registry insertion.

## 5. Frontend & sandbox lifecycle

The Kotlin `AppManager` calls three JNI entry points (`nativeScan`,
`nativeIcon`, `nativeLaunch`). A launch (`crates/jni/src/session.rs`):

1. reads `Info.plist` for metadata,
2. `Sandbox::prepare` builds `Applications/<UUID>/{.app,Documents,Library,tmp}`,
3. the `.app` payload is unzipped into it (zip-slip guarded),
4. the Mach-O is mapped into a `Machine` with the HLE environment, and
5. the run loop executes until the guest exits.

`Sandbox::translate` maps guest iOS-absolute paths into the container (writable)
or the shared read-only system image, rejecting any lexical escape before a host
path is ever constructed — the primary sandbox boundary.

## 6. Testing

The host suite (`cargo test`) covers the parts that need no CPU backend:
LEB128/bind decoding, Mach-O + FAT parsing (including a hand-assembled binary and
truncation fuzzing), binary & XML plist decoding, sandbox path translation and
escape rejection, memory protection/overlap enforcement, and a full
`Machine::load` → scripted-run → `exit` pipeline. The `ScriptedExecutor` lets the
dispatch loop be tested deterministically without a real CPU.
