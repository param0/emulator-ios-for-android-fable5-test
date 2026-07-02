/*
 * vm_bridge.c — reserves guest memory for a PIE Mach-O image.
 *
 * The guest is a position-independent executable, so its absolute link-time base
 * (0x100000000) is only a *preferred* address. On modern Android (esp. 16) that
 * range is not grantable and MAP_FIXED there fails or, worse, silently lands
 * elsewhere — after which a jump to a hardcoded entry point segfaults.
 *
 * The fix: do NOT request a fixed address. Reserve one contiguous span for the
 * whole image at an OS-chosen base and hand it back to Rust, which computes the
 * ASLR slide (chosen_base - preferred_base) and relocates the entry point, the
 * thread PC, and every segment placement by that slide. The region is mapped
 * writable; the loader flips each segment to its final protection afterwards.
 */
#include "ios_emu_jit.h"

#include <sys/mman.h>
#include <string.h>
#include <stdlib.h>
#include <errno.h>
#include <android/log.h>

#define IOS_EMU_LOG_TAG "ios-emu"

/* Translate our Protection bits (R=1,W=2,X=4) to mmap PROT_* flags. */
static int to_mmap_prot(uint32_t prot) {
    int p = 0;
    if (prot & 1) p |= PROT_READ;
    if (prot & 2) p |= PROT_WRITE;
    if (prot & 4) p |= PROT_EXEC;
    return p ? p : PROT_NONE;
}

/*
 * Reserve `len` bytes for the guest image at an OS-chosen base address.
 *
 * No MAP_FIXED: the kernel picks a free base, defeating the Android-16 block on
 * the preferred 0x100000000 range. Returns the base pointer; the caller derives
 * the ASLR slide from it. On failure there is no safe way to continue, so we log
 * the errno via <android/log.h> and abort() — a hard, greppable failure beats a
 * later SEGV_MAPERR with no context.
 */
void *ios_emu_native_map_region(size_t len) {
    if (len == 0) {
        __android_log_print(ANDROID_LOG_FATAL, IOS_EMU_LOG_TAG,
                            "ios_emu_native_map_region: refusing zero-length reservation");
        abort();
    }

    void *base = mmap(NULL, len, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) {
        __android_log_print(ANDROID_LOG_FATAL, IOS_EMU_LOG_TAG,
                            "mmap(NULL, %zu) failed: errno=%d (%s)",
                            len, errno, strerror(errno));
        abort();
    }

    __android_log_print(ANDROID_LOG_INFO, IOS_EMU_LOG_TAG,
                        "reserved image span: %zu bytes at %p", len, base);
    return base;
}

/*
 * Copy `src_len` bytes of segment file data to `dst` (a slid runtime address
 * inside the reservation). Kept separate from the reservation so Rust owns the
 * slide arithmetic. Bounded by `dst_len` (the segment's vmsize).
 */
void ios_emu_native_copy_in(void *dst, size_t dst_len, const void *src, size_t src_len) {
    if (src && src_len) {
        memcpy(dst, src, src_len < dst_len ? src_len : dst_len);
    }
}

/* Apply the final protection to a sub-range of the reservation. */
int ios_emu_native_protect(uint64_t addr, size_t len, uint32_t prot) {
    return mprotect((void *)(uintptr_t)addr, len, to_mmap_prot(prot));
}

/*
 * Publish freshly-written code in [begin, end) to the instruction cache after
 * writing it and marking the page executable. ARM64 I-cache and D-cache are NOT
 * coherent: stored instructions sit in the D-cache while the I-cache fetches
 * stale bytes, faulting as SIGILL/ILL_ILLOPC on the first jump. Call after
 * copying/patching loaded __TEXT and after filling the __stubs page, strictly
 * before executing.
 *
 * We deliberately do NOT use __builtin___clear_cache: on the NDK it can lower to
 * a call to the compiler-rt symbol `__clear_cache`, which libc.so does not
 * export, leaving libios_emu_jni.so with an unresolved dynamic symbol (dlopen ->
 * UnsatisfiedLinkError). Emitting the maintenance sequence inline references no
 * external symbol. This mirrors compiler-rt's own __clear_cache for AArch64.
 */
void ios_emu_native_clear_cache(void *start, size_t len) {
    /* Compute the end pointer internally so a caller can never pass a length
     * where an end pointer is expected: the range is [start, start + len). */
    char *begin = (char *)start;
    char *end = begin + len;
#if defined(__aarch64__)
    /* CTR_EL0 encodes the minimum D/I cache line sizes (log2 words). */
    uint64_t ctr;
    __asm__ __volatile__("mrs %0, ctr_el0" : "=r"(ctr));
    const size_t dcache_line = (size_t)4 << ((ctr >> 16) & 0xF);
    const size_t icache_line = (size_t)4 << (ctr & 0xF);

    /* Clean each D-cache line to the point of unification. */
    for (uintptr_t a = (uintptr_t)begin & ~(dcache_line - 1);
         a < (uintptr_t)end; a += dcache_line) {
        __asm__ __volatile__("dc cvau, %0" : : "r"(a) : "memory");
    }
    __asm__ __volatile__("dsb ish" : : : "memory");

    /* Invalidate each I-cache line to the point of unification. */
    for (uintptr_t a = (uintptr_t)begin & ~(icache_line - 1);
         a < (uintptr_t)end; a += icache_line) {
        __asm__ __volatile__("ic ivau, %0" : : "r"(a) : "memory");
    }
    __asm__ __volatile__("dsb ish" : : : "memory");
    __asm__ __volatile__("isb" : : : "memory");
#else
    /* Host / non-AArch64: caches are coherent or the builtin inlines safely. */
    __builtin___clear_cache(begin, end);
#endif
}

/* Release the whole reservation at teardown. */
int ios_emu_native_unmap(uint64_t addr, size_t len) {
    return munmap((void *)(uintptr_t)addr, len);
}
