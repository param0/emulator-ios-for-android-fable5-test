/*
 * vm_bridge.c — maps guest memory at its guest virtual addresses.
 *
 * For native execution the guest's address space must physically exist at the
 * guest addresses in this process (an identity mapping), so a guest load/store
 * to `0x1_0000_0000` hits real memory. The Rust `GuestMemory` remains the
 * authority on layout and contents; on device it drives these calls to
 * materialise each region, then copies the bytes in.
 *
 * Android note: mapping executable memory backed by app files requires the
 * mapping to be anonymous + copied (W^X is enforced; `PROT_EXEC` on a
 * file-backed writable mapping is denied). We therefore always map anonymous and
 * copy, flipping to the final protection afterwards.
 */
#include "ios_emu_jit.h"

#include <sys/mman.h>
#include <string.h>

/* Translate our Protection bits (R=1,W=2,X=4) to mmap PROT_* flags. */
static int to_mmap_prot(uint32_t prot) {
    int p = 0;
    if (prot & 1) p |= PROT_READ;
    if (prot & 2) p |= PROT_WRITE;
    if (prot & 4) p |= PROT_EXEC;
    return p ? p : PROT_NONE;
}

/*
 * Map [addr, addr+len) as anonymous memory at the fixed guest address and copy
 * `init_len` bytes from `init` into it (the rest stays zero-filled). Returns the
 * mapped address, or NULL on failure. The region is left writable; call
 * ios_emu_native_protect() after any patching to set the final protection.
 */
void *ios_emu_native_map_region(uint64_t addr, size_t len,
                                const void *init, size_t init_len) {
    void *hint = (void *)(uintptr_t)addr;
    void *p = mmap(hint, len, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (p == MAP_FAILED) {
        return 0;
    }
    if (init && init_len) {
        memcpy(p, init, init_len < len ? init_len : len);
    }
    return p;
}

/* Apply the final protection to a previously-mapped region. */
int ios_emu_native_protect(uint64_t addr, size_t len, uint32_t prot) {
    return mprotect((void *)(uintptr_t)addr, len, to_mmap_prot(prot));
}

/* Unmap a region at teardown. */
int ios_emu_native_unmap(uint64_t addr, size_t len) {
    return munmap((void *)(uintptr_t)addr, len);
}
