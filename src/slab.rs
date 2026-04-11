#[cfg(unix)]
use libc::{mmap, munmap, MAP_ANONYMOUS, MAP_HUGETLB, MAP_PRIVATE, PROT_READ, PROT_WRITE};
#[cfg(unix)]
use std::ptr::null_mut;

#[cfg(windows)]
use memmap2::MmapMut;

pub struct HugeSlab {
    #[cfg(unix)] ptr: *mut u8,
    #[cfg(unix)] size: usize,
    #[cfg(windows)] mmap: MmapMut,
}

unsafe impl Send for HugeSlab {}
unsafe impl Sync for HugeSlab {}

impl HugeSlab {
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "slab size must be non-zero");

        #[cfg(unix)]
        {
            let ptr = unsafe {
                let addr = mmap(null_mut(), size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
                if addr == libc::MAP_FAILED {
                    let fallback = mmap(null_mut(), size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
                    assert!(fallback != libc::MAP_FAILED, "mmap fallback failed");
                    fallback as *mut u8
                } else {
                    addr as *mut u8
                }
            };
            Self { ptr, size }
        }

        #[cfg(windows)]
        {
            // memmap2::MmapOptions::new().map_anon() is already marked as unsafe.
            // We just call it directly.
            let mmap = {
                memmap2::MmapOptions::new()
                    .len(size)
                    .map_anon()
                    .expect("Failed to allocate memory slab on Windows")
            };
            Self { mmap }
        }
    }

    #[inline(always)]
    pub fn ptr(&self) -> *mut u8 {
        #[cfg(unix)] { self.ptr }
        #[cfg(windows)] { self.mmap.as_ptr() as *mut u8 }
    }

    #[inline(always)]
    pub fn size(&self) -> usize {
        #[cfg(unix)] { self.size }
        #[cfg(windows)] { self.mmap.len() }
    }
}

impl Drop for HugeSlab {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            let ret = munmap(self.ptr as *mut _, self.size);
            debug_assert_eq!(ret, 0, "munmap failed");
        }
    }
}
