//! src/shm.rs
//!
//! Minimal shared‑memory wrapper that can be used by multiple processes
//! to access the same UltraSlayer slab.  The implementation is deliberately
//! simple – it creates (or opens) a file in `/dev/shm`, mmaps it with
//! `MAP_SHARED | MAP_HUGETLB` when huge pages are available, and then hands
//! the raw pointer to `UltraSlayer` for its internal operations.
//!
//! The public API mirrors the tiny subset that the README refers to:
//!   * `ShmSlab::<T>::create(name, channels, size_bytes)` – allocate a new slab.
//!   * `ShmSlab::<T>::open(name)` – open an existing slab created elsewhere.
//!   * `read(idx)`, `write(idx, val)` – volatile operations.
//!   * `into_ultraslayer(self) -> UltraSlayer<T>` – consume the wrapper and get the
//!     fully‑featured slab object.
//!
//! The code works on Linux only (uses `memfd_create`/`shm_open` and `mmap`).

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::slab::{UltraSlayer, SpinPolicy};

/// Helper to create a *named* shared‑memory object.  On most Linux systems
/// `/dev/shm/<name>` is a `tmpfs` mount that is perfect for this purpose.
fn shm_file_path(name: &str) -> Result<std::fs::File> {
    // Ensure the file exists – create it if needed.
    let path = Path::new("/dev/shm").join(name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)?;
    Ok(file)
}

/// The public wrapper.  It holds the raw mmap pointer and the length, and
/// forwards all operations to an inner `UltraSlayer<T>`.
pub struct ShmSlab<T> {
    // The mmap’d region.  `NonNull<T>` guarantees the pointer is non‑null.
    ptr: NonNull<T>,
    // Length *in elements* (not bytes).  The slab size is expressed in
    // bytes during construction; we translate it to a number of `T`s.
    len: usize,
    // The file descriptor that backs the mapping – kept alive so the
    // mapping stays valid.
    _file: std::fs::File,
    // The underlying UltraSlayer (mirrored, spin‑policy, etc.).
    slayer: UltraSlayer<T>,
}

impl<T> ShmSlab<T>
where
    T: Copy + Send + Sync + 'static,
{
    /// Create a brand‑new shared slab.
    ///
    /// * `name` – a unique identifier (e.g. `"ultra_slab"`).  
    /// * `channels` – how many DRAM mirrors you want.  
    /// * `size_bytes` – total size of the slab (the same size is allocated
    ///   for each channel, *not* divided by `channels`).
    pub fn create(name: &str, channels: usize, size_bytes: usize) -> Result<Self> {
        let file = shm_file_path(name)?;
        // Ensure the file is large enough.
        file.set_len(size_bytes as u64)?;

        // Map the region as shared, using huge pages if the kernel can give them.
        // `MAP_HUGETLB` is optional – if the system cannot supply a huge page
        // the call still succeeds and falls back to normal pages.
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let flags = libc::MAP_SHARED
            | libc::MAP_LOCKED
            | libc::MAP_POPULATE
            | libc::MAP_HUGETLB; // may be ignored if not available

        let raw_ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_bytes,
                prot,
                flags,
                file.as_raw_fd(),
                0,
            )
        };

        if raw_ptr == libc::MAP_FAILED {
            return Err(Error::last_os_error());
        }

        // SAFETY: we just mapped `size_bytes` bytes for a `T` array and we
        // verified `size_bytes % size_of::<T>() == 0` in the slab constructor.
        let ptr = NonNull::new(raw_ptr as *mut T).ok_or_else(|| {
            Error::new(ErrorKind::Other, "mmap returned a null pointer")
        })?;

        // Initialise the UltraSlayer on top of the raw memory.
        let slayer = UltraSlayer::<T>::with_raw(ptr, size_bytes, channels)?;

        Ok(Self {
            ptr,
            len: size_bytes / std::mem::size_of::<T>(),
            _file: file,
            slayer,
        })
    }

    /// Open an already‑existing shared slab.
    pub fn open(name: &str, channels: usize, size_bytes: usize) -> Result<Self> {
        let file = shm_file_path(name)?;
        let raw_ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if raw_ptr == libc::MAP_FAILED {
            return Err(Error::last_os_error());
        }
        let ptr = NonNull::new(raw_ptr as *mut T).ok_or_else(|| {
            Error::new(ErrorKind::Other, "mmap returned a null pointer")
        })?;
        let slayer = UltraSlayer::<T>::with_raw(ptr, size_bytes, channels)?;
        Ok(Self {
            ptr,
            len: size_bytes / std::mem::size_of::<T>(),
            _file: file,
            slayer,
        })
    }

    /// Consume the wrapper and hand ownership of the UltraSlayer object to the
    /// caller.  After this call the `ShmSlab` is dropped and the mmap
    /// will be unmapped – **use only when you are sure no other process
    /// still needs the memory**.
    pub fn into_ultraslayer(self) -> UltraSlayer<T> {
        self.slayer
    }

    // -----------------------------------------------------------------
    // Simple volatile accessors that forward to the inner UltraSlayer.
    // -----------------------------------------------------------------
    #[inline(always)]
    pub fn read(&self, idx: usize) -> T {
        self.slayer.read(idx)
    }

    #[inline(always)]
    pub fn write(&self, idx: usize, val: T) {
        self.slayer.write(idx, val);
    }

    // -----------------------------------------------------------------
    // Optional helper to change the spin policy *after* creation.
    // -----------------------------------------------------------------
    #[inline(always)]
    pub fn set_spin_policy(&self, policy: SpinPolicy) {
        self.slayer.set_spin_policy(policy);
    }

    // -----------------------------------------------------------------
    // Return the number of elements the slab can hold.
    // -----------------------------------------------------------------
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }
}

// Unmap the region when the wrapper is dropped.
impl<T> Drop for ShmSlab<T> {
    fn drop(&mut self) {
        let size_bytes = self.len * std::mem::size_of::<T>();
        unsafe {
            libc::munmap(self.ptr.as_ptr() as *mut _, size_bytes);
        }
    }
}
