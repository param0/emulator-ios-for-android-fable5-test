# iOS 7 (AArch64) Emulator for Android

A modular, extensible emulator that runs 64-bit iOS 7 applications on AArch64
Android devices. It executes guest ARM64 code **natively** (Android is also
ARM64), intercepting Darwin syscalls and framework calls and servicing them with
a High-Level-Emulation (HLE) layer written in safe Rust.

> **Status:** research/bring-up. The loader, VFS, memory manager, syscall
> dispatch, Obj-C runtime bridge, and IPA/plist pipeline are implemented and unit
> tested on the host (`cargo test`, 27 tests). The native AArch64 execution
> backend and the Android UI compile against the NDK/SDK (not buildable in a
> headless CI container without them). See [Component status](#component-status).

---

## Design in one paragraph

Because the host CPU is ARM64, we do **not** interpret or JIT-recompile in the
common case — we run the guest's real instructions. Two things are trapped: a
Darwin `svc #0x80` (rewritten to `brk` at load time so it traps to us instead of
the Linux kernel) and a branch into the **import trampoline page** (an imported
framework symbol). Each trap returns control to a small Rust dispatch loop, which
services it via the [syscall table](crates/core/src/syscall/) or the
[HLE framework registry](crates/hle/src/frameworks/) and resumes. All memory
layout, path translation, and library behaviour live in safe Rust; only the
register-install/trap-capture backend is C + a little inline assembly.

```
 guest ARM64 ─run natively─▶ svc→brk / call into __stubs ─trap─▶ signal handler
        ▲                                                              │
        └────────────── resume (pc = lr / pc+4) ◀── Rust dispatch ◀────┘
                                                   (syscalls · HLE frameworks)
```

## Directory tree

```
.
├── Cargo.toml                  # Rust workspace (DAG of small crates)
├── rust-toolchain.toml
├── .cargo/config.toml          # Android target linker configuration
├── crates/
│   ├── common/                 # shared types: GuestAddr, Protection, EmuError
│   ├── memory/                 # guest address space: regions, protections, heap
│   ├── loader/                 # Mach-O + FAT parser, dyld bind-opcode parser
│   │   ├── src/macho/          #   header / load commands / segments / reader
│   │   └── src/dyld/           #   bind info + symbol→trampoline resolver
│   ├── vfs/                    # per-app sandbox + iOS→host path translation
│   ├── ipa/                    # IPA discovery, binary+XML plist parser, unzip
│   ├── core/                   # CPU context, executor trait, syscall dispatch,
│   │   └── src/syscall/        #   process construction & the run loop
│   ├── hle/                    # Objective-C runtime + framework shims
│   │   ├── src/objc/           #   objc_msgSend dispatcher
│   │   └── src/frameworks/     #   libSystem, Foundation, UIKit, CoreGraphics, GLES
│   └── jni/                    # cdylib: pure-Rust session API + JNI bridge
├── native/                     # C/C++ low-level backend
│   ├── include/ios_emu_jit.h   #   Rust⇄C ABI (register context, events)
│   └── src/
│       ├── jit_exec.c          #   native resume + trap-based syscall capture
│       └── vm_bridge.c         #   mmap guest memory at fixed guest addresses
└── frontend/                   # Android app (Kotlin + Jetpack Compose)
    └── app/src/main/
        ├── java/com/iosemu/emulator/
        │   ├── emu/EmulatorBridge.kt   # external (JNI) declarations
        │   ├── AppManager.kt           # scan / icon / launch orchestration
        │   ├── model/AppEntry.kt
        │   └── ui/MainActivity.kt      # Compose app grid
        └── AndroidManifest.xml
```

## Crate dependency graph (a strict DAG)

```
common ─▶ memory ─┐
       ─▶ loader ─┼─▶ core ─▶ hle ─┐
       ─▶ vfs ────┘                ├─▶ jni (cdylib)
       ─▶ ipa ──────────────────────┘
```

`core` never depends on `hle`; instead it defines the
[`HostEnvironment`](crates/core/src/env.rs) trait that `hle` implements, so the
core loop is testable in isolation and the graph stays acyclic.

## Building & testing

### Host (logic that needs no device)

```bash
cargo test          # 27 unit/integration tests: loader, plist, vfs, memory,
                    # syscall dispatch, and a full Machine::load pipeline
cargo build         # builds every crate for the host
```

The `ios-emu-jni` crate's `build.rs` compiles the C backend **only** for Android
targets, so host builds need no NDK.

### Android device (arm64)

Prerequisites: Android SDK + NDK r26, `rustup target add aarch64-linux-android`,
and `cargo install cargo-ndk` (the Gradle Rust plugin uses the NDK toolchains).

```bash
# 1. Configure the NDK linker (edit .cargo/config.toml or export ANDROID_NDK_HOME)
# 2. Build the APK — Gradle runs `cargoBuild` to produce libios_emu_jni.so:
cd frontend
./gradlew :app:assembleRelease
```

Push `.ipa` files to the app's external files dir
(`Android/data/com.iosemu.emulator/files/IPAs/`) and launch from the grid.

## Component status

| Requirement | Where | State |
|---|---|---|
| IPA discovery + `Info.plist` (binary & XML) | `crates/ipa` | **implemented** |
| App metadata + icon extraction | `crates/ipa/discovery.rs` | **implemented** |
| Per-app sandbox + path translation | `crates/vfs` | **implemented** |
| 64-bit Mach-O + FAT parser | `crates/loader/macho` | **implemented** |
| dyld bind-opcode parsing + import resolution | `crates/loader/dyld` | **implemented** |
| Guest memory: regions, protections, heap, mmap | `crates/memory` | **implemented** |
| Process construction (map, bind, stack, entry) | `crates/core/loader.rs` | **implemented** |
| Darwin/XNU syscall translation + Mach traps | `crates/core/syscall` | **core set + safe stubs** |
| Native AArch64 execution + trap capture | `native/` | **implemented (device only)** |
| `objc_msgSend` runtime bridge | `crates/hle/objc` | **implemented (common idioms)** |
| Foundation / UIKit / CoreGraphics / GLES | `crates/hle/frameworks` | **modular shims + stubs** |
| Android frontend / JNI | `frontend`, `crates/jni` | **implemented** |

Everything not yet modelled is *explicitly stubbed*: each unimplemented syscall
or framework function logs its invocation (`logcat` tag `ios-emu`, target
`ios_emu::stub`) and returns a safe default, so an app keeps running instead of
crashing on first contact with an unimplemented API.

## Extending

* **A new syscall:** add its number to `crates/core/src/syscall/numbers.rs` and a
  handler arm to `unix.rs` / `mach.rs`. No other file changes.
* **A new framework function:** write one `fn(&mut ObjcRuntime, &mut CallContext)`
  shim and register it in the relevant `crates/hle/src/frameworks/*.rs`. Register
  a name to `stub_zero` if you only need it to not crash yet.
* **A whole framework:** add a module under `frameworks/` and one line to
  `install_all`.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the detailed design,
including the trap protocol, the slide/binding policy, and the ABI between Rust
and the native backend.

## Legal

For running software you are licensed to run. App Store binaries ship with
FairPlay-encrypted `__TEXT`; the loader detects this and refuses
(`is_encrypted()`), so a decrypted build is required. This project ships no Apple
code — the frameworks are re-implemented, not redistributed.
