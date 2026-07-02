/*
 * ios_emu_jit.h — ABI between the Rust core and the native AArch64 backend.
 *
 * Because Android is itself AArch64, guest iOS code is executed *natively*: the
 * backend installs the guest register file and branches into guest text. Two
 * things must be intercepted, since both would otherwise do the wrong thing when
 * run directly:
 *
 *   - `svc #0x80` (a Darwin syscall) would trap into the *Linux* kernel with a
 *     Darwin syscall number. At load time these are rewritten to `brk` so they
 *     trap to our signal handler instead.
 *   - a call to an imported library function branches into the trampoline page,
 *     which is filled with `brk`, trapping likewise.
 *
 * The signal handler snapshots the CPU state into `guest_cpu_context`, decodes
 * the trap, and returns control to `ios_emu_native_resume`, which reports the
 * event to Rust. Rust services it (syscall table / HLE) and calls resume again.
 */
#ifndef IOS_EMU_JIT_H
#define IOS_EMU_JIT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Mirror of `ios_emu_core::cpu::CpuContext` (which is `#[repr(C)]`). Field order
 * and sizes MUST stay in lock-step with the Rust definition.
 */
typedef struct {
    uint64_t x[29];      /* x0..x28                                   */
    uint64_t fp;         /* x29                                       */
    uint64_t lr;         /* x30                                       */
    uint64_t sp;         /* stack pointer                             */
    uint64_t pc;         /* program counter                          */
    uint64_t nzcv;       /* condition flags (N,Z,C,V in bits 31..28)  */
    uint64_t tpidrro_el0;/* thread pointer / TLS base                 */
} guest_cpu_context;

/* Event discriminants returned by ios_emu_native_resume (match the Rust match). */
enum ios_emu_event {
    IOS_EMU_EVENT_SYSCALL      = 0, /* out_imm = svc immediate (0x80)          */
    IOS_EMU_EVENT_TRAMPOLINE   = 1, /* out_addr = trampoline address hit       */
    IOS_EMU_EVENT_BREAKPOINT   = 2, /* out_imm = brk immediate                 */
    IOS_EMU_EVENT_EXITED       = 3, /* out_code = process exit code            */
    IOS_EMU_EVENT_FAULT        = 4  /* unrecoverable                           */
};

/*
 * One-time process setup: installs the SIGTRAP/SIGSEGV/SIGILL handlers used to
 * trap guest breakpoints and faults. Returns 0 on success.
 */
int ios_emu_native_init(void);

/*
 * Publish the [lo, hi) address range of the import-trampoline page so the trap
 * handler can classify a branch into it as an imported-function call.
 */
void ios_emu_native_set_trampoline_range(uint64_t lo, uint64_t hi);

/*
 * Rewrite every `svc #0x80` in [addr, addr+len) to a `brk` with the syscall
 * marker, then flush the i-cache for that range. Call on each executable segment
 * after it is mapped and before it is executed.
 */
void ios_emu_native_prepare_text(void *addr, size_t len);

/*
 * Reserve `len` bytes of anonymous rw- memory at an OS-chosen base (no
 * MAP_FIXED); aborts on failure. Backs the PIE image span and the stub page.
 */
void *ios_emu_native_map_region(size_t len);

/* Apply the final protection (R/W/X bits) to a sub-range of a reservation. */
int ios_emu_native_protect(uint64_t addr, size_t len, uint32_t prot);

/*
 * Publish freshly-written code in [begin, end) to the instruction cache (clean
 * D-cache to PoU + invalidate I-cache). MUST be called after copying/patching an
 * executable region and before the first jump into it, or the CPU fetches stale
 * bytes and faults with SIGILL/ILL_ILLOPC. Implemented with inline AArch64 cache
 * ops so it references no external symbol (avoids the unexported `__clear_cache`
 * that `__builtin___clear_cache` can emit).
 */
void ios_emu_native_clear_cache(char *begin, char *end);

/*
 * Install `ctx` into the real CPU and resume guest execution at `ctx->pc` until
 * the next trap. On return the out-params carry the event payload and `ctx`
 * holds the guest state at the trap. Returns an `ios_emu_event` discriminant.
 */
uint32_t ios_emu_native_resume(guest_cpu_context *ctx,
                               uint16_t *out_imm,
                               uint64_t *out_addr,
                               int32_t *out_code);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* IOS_EMU_JIT_H */
