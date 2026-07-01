/*
 * jit_exec.c — native AArch64 execution backend.
 *
 * See ios_emu_jit.h for the architecture. The interesting parts:
 *
 *   - `prepare_text` patches `svc #0x80` -> `brk #SYSCALL_MARKER` so Darwin
 *     syscalls trap to us rather than the Linux kernel.
 *   - `resume` uses `sigsetjmp` to establish a return point, then a small asm
 *     stub loads the guest register file and branches into guest code.
 *   - the SIGTRAP handler snapshots `mcontext` into the shared context, decodes
 *     whether the trap is a syscall, a trampoline call, or a real breakpoint,
 *     and `siglongjmp`s back into `resume`.
 *
 * The AArch64-specific pieces are guarded by `__aarch64__`; on any other host
 * the file still compiles (so the scaffolding is checkable) but `resume`
 * reports a fault.
 */
#define _GNU_SOURCE /* expose sigjmp_buf / siglongjmp under strict -std=c11 */
#include "ios_emu_jit.h"

#include <setjmp.h>
#include <signal.h>
#include <string.h>

#if defined(__linux__)
#include <ucontext.h>
#endif

/* AArch64 fixed-width instruction encodings we care about. */
#define INSN_SVC_0x80 0xD4001001u          /* svc #0x80                        */
#define BRK_BASE 0xD4200000u               /* brk #0  (imm goes in bits 20..5)  */
#define SYSCALL_MARKER 0x00A2u             /* brk immediate used for syscalls   */

#define BRK_MASK 0xFFE0001Fu               /* isolates the brk opcode           */
#define brk_encode(imm) (BRK_BASE | (((uint32_t)(imm) & 0xFFFFu) << 5))
#define is_brk(insn) (((insn) & BRK_MASK) == BRK_BASE)
#define brk_imm(insn) (uint16_t)(((insn) >> 5) & 0xFFFFu)

/* ---- backend state (single guest thread) -------------------------------- */

static uint64_t g_tramp_lo = 0;
static uint64_t g_tramp_hi = 0;

void ios_emu_native_set_trampoline_range(uint64_t lo, uint64_t hi) {
    g_tramp_lo = lo;
    g_tramp_hi = hi;
}

void ios_emu_native_prepare_text(void *addr, size_t len) {
    uint32_t *code = (uint32_t *)addr;
    size_t n = len / sizeof(uint32_t);
    for (size_t i = 0; i < n; i++) {
        if (code[i] == INSN_SVC_0x80) {
            code[i] = brk_encode(SYSCALL_MARKER);
        }
    }
#if defined(__GNUC__)
    /* Make the rewritten instructions visible to the i-cache. */
    __builtin___clear_cache((char *)addr, (char *)addr + len);
#endif
}

/* ---- trap handling ------------------------------------------------------ */

#if defined(__aarch64__) && defined(__linux__)

/* Per-resume return point and out-param plumbing, read by the signal handler. */
static sigjmp_buf g_return;
static guest_cpu_context *g_ctx;
static uint16_t *g_out_imm;
static uint64_t *g_out_addr;
static int32_t *g_out_code;
static volatile uint32_t g_event;

/* Snapshot the kernel mcontext into the shared guest context. */
static void capture(const mcontext_t *mc, guest_cpu_context *ctx) {
    for (int i = 0; i < 29; i++) {
        ctx->x[i] = mc->regs[i];
    }
    ctx->fp = mc->regs[29];
    ctx->lr = mc->regs[30];
    ctx->sp = mc->sp;
    ctx->pc = mc->pc;
    ctx->nzcv = mc->pstate & 0xF0000000u;
}

static void trap_handler(int sig, siginfo_t *info, void *uc) {
    (void)info;
    ucontext_t *ctx = (ucontext_t *)uc;
    mcontext_t *mc = &ctx->uc_mcontext;

    capture(mc, g_ctx);

    if (sig == SIGSEGV || sig == SIGBUS) {
        g_event = IOS_EMU_EVENT_FAULT;
        siglongjmp(g_return, 1);
    }

    /* Classify the breakpoint by PC range first, then by decoded immediate. */
    uint64_t pc = mc->pc;
    if (pc >= g_tramp_lo && pc < g_tramp_hi) {
        *g_out_addr = pc;
        g_event = IOS_EMU_EVENT_TRAMPOLINE;
        siglongjmp(g_return, 1);
    }

    uint32_t insn = *(const uint32_t *)pc;
    if (is_brk(insn)) {
        uint16_t imm = brk_imm(insn);
        if (imm == SYSCALL_MARKER) {
            *g_out_imm = 0x80;
            g_event = IOS_EMU_EVENT_SYSCALL;
            /* Advance past the trap so the next resume continues after it. */
            g_ctx->pc = pc + 4;
        } else {
            *g_out_imm = imm;
            g_event = IOS_EMU_EVENT_BREAKPOINT;
        }
        siglongjmp(g_return, 1);
    }

    g_event = IOS_EMU_EVENT_FAULT;
    siglongjmp(g_return, 1);
}

/*
 * Load the guest register file and branch to `ctx->pc`. Never returns normally;
 * control leaves via a trap -> signal handler -> siglongjmp. `x18` is the
 * platform-reserved register on both Android and iOS, so it is safe to use as
 * the scratch that holds the branch target.
 *
 * Offsets are the byte offsets of guest_cpu_context fields (verified by the
 * _Static_assert block below).
 */
__attribute__((noreturn)) static void install_and_branch(guest_cpu_context *ctx) {
    __asm__ __volatile__(
        "mov x18, %0            \n" /* x18 = ctx base                         */
        "ldr x1, [x18, #248]    \n" /* sp                                     */
        "mov sp, x1             \n"
        "ldr x1, [x18, #264]    \n" /* nzcv                                   */
        "msr nzcv, x1           \n"
        /* Load x0..x17 and x19..x28 from ctx->x[]. */
        "ldp x0,  x1,  [x18, #0]   \n"
        "ldp x2,  x3,  [x18, #16]  \n"
        "ldp x4,  x5,  [x18, #32]  \n"
        "ldp x6,  x7,  [x18, #48]  \n"
        "ldp x8,  x9,  [x18, #64]  \n"
        "ldp x10, x11, [x18, #80]  \n"
        "ldp x12, x13, [x18, #96]  \n"
        "ldp x14, x15, [x18, #112] \n"
        "ldp x16, x17, [x18, #128] \n"
        "ldp x19, x20, [x18, #152] \n" /* skip x18 slot at 144               */
        "ldp x21, x22, [x18, #168] \n"
        "ldp x23, x24, [x18, #184] \n"
        "ldp x25, x26, [x18, #200] \n"
        "ldp x27, x28, [x18, #216] \n"
        "ldr x29, [x18, #232]   \n" /* fp                                     */
        "ldr x30, [x18, #240]   \n" /* lr                                     */
        "ldr x18, [x18, #256]   \n" /* pc -> scratch (base now dead)          */
        "br  x18                \n"
        :
        : "r"(ctx)
        : "memory");
    __builtin_unreachable();
}

/* Compile-time verification that the asm offsets above match the struct. */
_Static_assert(offsetof(guest_cpu_context, x) == 0, "x offset");
_Static_assert(offsetof(guest_cpu_context, fp) == 232, "fp offset");
_Static_assert(offsetof(guest_cpu_context, lr) == 240, "lr offset");
_Static_assert(offsetof(guest_cpu_context, sp) == 248, "sp offset");
_Static_assert(offsetof(guest_cpu_context, pc) == 256, "pc offset");
_Static_assert(offsetof(guest_cpu_context, nzcv) == 264, "nzcv offset");

int ios_emu_native_init(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = trap_handler;
    sa.sa_flags = SA_SIGINFO | SA_NODEFER;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGTRAP, &sa, NULL) != 0) return -1;
    if (sigaction(SIGILL, &sa, NULL) != 0) return -1;
    if (sigaction(SIGSEGV, &sa, NULL) != 0) return -1;
    if (sigaction(SIGBUS, &sa, NULL) != 0) return -1;
    return 0;
}

uint32_t ios_emu_native_resume(guest_cpu_context *ctx, uint16_t *out_imm,
                               uint64_t *out_addr, int32_t *out_code) {
    g_ctx = ctx;
    g_out_imm = out_imm;
    g_out_addr = out_addr;
    g_out_code = out_code;
    g_event = IOS_EMU_EVENT_FAULT;

    if (sigsetjmp(g_return, 1) == 0) {
        install_and_branch(ctx); /* branches into guest; returns via longjmp */
    }
    return g_event; /* set by the trap handler */
}

#else /* -------- non-AArch64 host: honest stub so the file still builds ---- */

int ios_emu_native_init(void) { return 0; }

void ios_emu_native_prepare_text_stub(void) { /* keep symbol set non-empty */ }

uint32_t ios_emu_native_resume(guest_cpu_context *ctx, uint16_t *out_imm,
                               uint64_t *out_addr, int32_t *out_code) {
    (void)ctx;
    (void)out_imm;
    (void)out_addr;
    (void)out_code;
    /* The native backend requires an AArch64 host (i.e. an Android device). */
    return IOS_EMU_EVENT_FAULT;
}

#endif
