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
    use std::os::raw::{c_int, c_ulong, c_uint};
    use std::sync::Arc;
    use crate::reader::{UltraSlayer, SpinPolicy};

    // We define Handle as the raw pointer to the Arc.
    // This is what we pass back and forth to the C side.
    type Handle = UltraSlayer<u64>;

    #[no_mangle]
    pub extern "C" fn ul_init(channels: c_uint, size_bytes: c_ulong) -> *mut Handle {
        let channels = channels as usize;
        let size_bytes = size_bytes as usize;
        
        if size_bytes == 0 || size_bytes % std::mem::size_of::<u64>() != 0 {
            return std::ptr::null_mut();
        }

        // UltraSlayer::new returns the object directly, not a Result.
        let sl = UltraSlayer::<u64>::new(channels, size_bytes);

        // Convert Arc to raw pointer to pass to C.
        Arc::into_raw(Arc::new(sl)) as *mut Handle
    }

    #[no_mangle]
    pub extern "C" fn ul_start_core(handle: *mut Handle) -> c_int {
        if handle.is_null() {
            return -1;
        }
        // SAFETY: Recover reference from the raw pointer.
        let sl = unsafe { &*handle };
        sl.spawn_slayer_core(0);
        0
    }

    #[no_mangle]
    pub extern "C" fn ul_set_spin_policy(handle: *mut Handle, policy: c_int) -> c_int {
        if handle.is_null() {
            return -1;
        }
        let sl = unsafe { &*handle };
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
        let sl = unsafe { &*handle };
        sl.read(idx as usize) as c_ulong
    }

    #[no_mangle]
    pub extern "C" fn ul_write_u64(handle: *mut Handle, idx: c_ulong, val: c_ulong) {
        if handle.is_null() {
            return;
        }
        let sl = unsafe { &*handle };
        sl.insert(idx as usize, val as u64);
    }

    #[no_mangle]
    pub extern "C" fn ul_destroy(handle: *mut Handle) {
        if !handle.is_null() {
            // SAFETY: Reconstruct the Arc to let it drop and free the memory.
            unsafe {
                let _ = Arc::from_raw(handle);
            }
        }
    }
}
