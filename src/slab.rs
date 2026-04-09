use libc::{mmap, munmap, MAP_ANONYMOUS, MAP_HUGETLB, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use std::ptr::null_mut;

/// A huge-page-backed memory slab.
///
/// Uses 2 MiB huge pages to eliminate TLB pressure at scale. Falls back to
/// standard 4 KiB pages if the OS has no pre-allocated huge pages
/// (`/proc/sys/vm/nr_hugepages`).
///
/// # Safety invariants
/// - `ptr` is always a valid, non-null, readable and writable mapping of
///   exactly `size` bytes for the lifetime of this struct.
/// - The mapping is zero-initialised by the kernel on allocation.
/// - `HugeSlab` is `Send` and `Sync` because callers are responsible for
///   synchronising access to the bytes within (see `UltraSlayer`).
pub struct HugeSlab {
    ptr: *mut u8,
    size: usize,
}

// SAFETY: The raw pointer is owned exclusively by this struct. Callers
// must enforce their own synchronisation over the slab's memory, which
// UltraSlayer does via its signalling protocol and exclusive write access
// during insert.
unsafe impl Send for HugeSlab {}
unsafe impl Sync for HugeSlab {}

impl HugeSlab {
    /// Allocates a slab of `size` bytes.
    ///
    /// # Panics
    /// Panics if the OS refuses both the huge-page and the fallback allocation.
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "slab size must be non-zero");

        let ptr = unsafe {
            // Attempt 2 MiB huge page allocation.
            let addr = mmap(
                null_mut(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB,
                -1,
                0,
            );

            if addr == libc::MAP_FAILED {
                // Huge pages unavailable: fall back to standard pages.
                let fallback = mmap(
                    null_mut(),
                    size,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS,
                    -1,
                    0,
                );
                assert!(
                    fallback != libc::MAP_FAILED,
                    "mmap fallback failed: both huge-page and standard allocations refused by OS"
                );
                fallback as *mut u8
            } else {
                addr as *mut u8
            }
        };

        // ptr is guaranteed non-null and valid here; MAP_FAILED is checked above.
        Self { ptr, size }
    }

    /// Returns the base pointer of the slab.
    ///
    /// # Safety
    /// The caller must ensure that any derived pointer stays within
    /// `[ptr, ptr + size)` and that concurrent accesses are properly
    /// synchronised.
    #[inline(always)]
    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Returns the total byte capacity of the slab.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for HugeSlab {
    fn drop(&mut self) {
        unsafe {
            let ret = munmap(self.ptr as *mut _, self.size);
            debug_assert_eq!(ret, 0, "munmap failed");
        }
    }
}
