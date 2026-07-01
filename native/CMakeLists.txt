# Native low-level backend for the iOS-on-Android emulator.
#
# Produces `libios_emu_native.so`, statically absorbed into the Rust cdylib on
# device (the Rust `NativeExecutor` binds `ios_emu_native_resume` over FFI). The
# Android Gradle plugin invokes this via `externalNativeBuild`.
cmake_minimum_required(VERSION 3.22)
project(ios_emu_native C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_CXX_STANDARD 17)

add_library(ios_emu_native STATIC
    src/jit_exec.c
    src/vm_bridge.c
)

target_include_directories(ios_emu_native PUBLIC include)

# The trap-based backend needs frame pointers preserved for reliable unwinding
# out of the signal handler, and no PLT indirection on the hot resume path.
target_compile_options(ios_emu_native PRIVATE
    -O2
    -fno-omit-frame-pointer
    -Wall
    -Wextra
)

# On device, link against the Android log library for any C-side diagnostics.
if(ANDROID)
    find_library(log-lib log)
    target_link_libraries(ios_emu_native ${log-lib})
endif()
