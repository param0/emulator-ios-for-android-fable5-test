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
 * Flush the instruction cache for [addr, addr+len) after writing code there and
 * marking it executable. ARM64 I-cache and D-cache are NOT coherent: freshly
 * stored instructions sit in the D-cache while the I-cache fetches stale bytes,
 * which faults as SIGILL/ILL_ILLOPC. __builtin___clear_cache emits the required
 * publish sequence (dc cvau over the range, dsb ish, ic ivau, dsb ish, isb).
 * Call after copying/patching loaded __TEXT and after filling the __stubs page,
 * strictly before the first jump into that code.
 */
void ios_emu_native_flush_icache(uint64_t addr, size_t len) {
    char *begin = (char *)(uintptr_t)addr;
    __builtin___clear_cache(begin, begin + len);
}

/* Release the whole reservation at teardown. */
int ios_emu_native_unmap(uint64_t addr, size_t len) {
    return munmap((void *)(uintptr_t)addr, len);
}
