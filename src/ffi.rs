//! src/ffi.rs
//!
//! A thin C‑FFI surface that lets other languages load UltraSlayer as a
//! shared library (`libultraslayer.so`).  The functions are deliberately
//! minimal – they create a slab of `u64`, start the background core, perform
//! volatile reads/writes, and finally destroy the slab.
//!
//! The file is compiled **only** when the `sidecar` Cargo feature is enabled.

#[cfg(feature = "sidecar")]
mod ffi {
    use std::os::raw::{c_char, c_int, c_ulong, c_uint, c_void};
    use std::ffi::CStr;
    use std::ptr::null_mut;
    use std::sync::Arc;

    use crate::slab::{UltraSlayer, SpinPolicy};

    // We store the UltraSlayer inside an `Arc` so that the C code can hold a
    // raw pointer (`*mut UltraSlayer<u64>`) without worrying about ownership.
    // The pointer returned from `ul_init` is later handed back to `Box::from_raw`
    // inside `ul_destroy`.
    type Handle = Arc<UltraSlayer<u64>>;

    #[no_mangle]
    pub extern "C" fn ul_init(
        channels: c_uint,
        size_bytes: c_ulong,
    ) -> *mut Handle {
        // Safety: the caller must ensure `size_bytes` > 0 and divisible by
        // `size_of::<u64>()`.  If the request is bogus we simply return null.
        if size_bytes == 0 || size_bytes % std::mem::size_of::<u64>() != 0 {
            return null_mut();
        }

        // Create the slab (no shared memory – just a normal allocation for the
        // sidecar use‑case).  Errors are turned into null pointers.
        let sl = match UltraSlayer::<u64>::with_channels(channels as usize, size_bytes as usize) {
            Ok(s) => s,
            Err(_) => return null_mut(),
        };

        Arc::into_raw(Arc::new(sl)) as *mut Handle
    }

    #[no_mangle]
    pub extern "C" fn ul_start_core(handle: *mut Handle) -> c_int {
        if handle.is_null() {
            return -1;
        }
        // SAFETY: we are guaranteed that the pointer came from `Arc::into_raw`.
        let sl = unsafe { &*(*handle) };
        sl.spawn_slayer_core();
        0
    }

    #[no_mangle]
    pub extern "C" fn ul_set_spin_policy(handle: *mut Handle, policy: c_int) -> c_int {
        if handle.is_null() {
            return -1;
        }
        let sl = unsafe { &*(*handle) };
        let sp = match policy {
            0 => SpinPolicy::Busy,
            1 => SpinPolicy::HybridYield,
            2 => SpinPolicy::Sleep,
            _ => return -1,
        };
        sl.set_spin_policy(sp);
        0
    }

    #[no_mangle]
    pub extern "C" fn ul_read_u64(handle: *mut Handle, idx: c_ulong) -> c_ulong {
        if handle.is_null() {
            return 0;
        }
        let sl = unsafe { &*(*handle) };
        sl.read(idx as usize) as c_ulong
    }

    #[no_mangle]
    pub extern "C" fn ul_write_u64(handle: *mut Handle, idx: c_ulong, val: c_ulong) {
        if handle.is_null() {
            return;
        }
        let sl = unsafe { &*(*handle) };
        sl.write(idx as usize, val as u64);
    }

    #[no_mangle]
    pub extern "C" fn ul_destroy(handle: *mut Handle) {
        if handle.is_null() {
            return;
        }
        // Re‑create the Arc so it gets dropped automatically.
        unsafe {
            Arc::from_raw(*handle);
        }
    }
}
