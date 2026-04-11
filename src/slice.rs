//! src/slice.rs
//!
//! Zero‑copy view into an `UltraSlayer<T>` slab.
//!
//! The view is deliberately **minimal** – it only stores a raw pointer and a length.
//! All safe operations (`as_slice`, `as_mut_slice`, `len`, `is_empty`) are
//! `unsafe` because the underlying slab is *mirrored* and may be mutated
//! concurrently by the Slayer Core.  Users who need a safe read‑only view
//! should call `as_slice()` inside an `unsafe` block and treat the result as
//! read‑only (the core only ever writes whole‑cache‑line sized chunks, so
//! reading the slice is race‑free).  For write‑only use‑cases the crate
//! provides `UltraSlayer::write` directly, so `Slice` is primarily for
//! bulk‑read scenarios (e.g. dumping a whole price book to a network buffer).

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

/// `Slice<'a, T>` is a thin wrapper around a raw pointer/length pair that
/// represents a *view* into an `UltraSlayer<T>` slab.  The lifetime `'a` ties the
/// view to the slab that produced it – you cannot keep a `Slice` after the slab
/// has been dropped.
///
/// # Safety
///
/// The struct itself does **not** guarantee that the memory it points to is
/// initialized or that concurrent writes are impossible.  All functions that
/// expose a `&[T]` or `&mut [T]` are marked `unsafe` and the caller must
/// uphold the usual Rust aliasing rules:
///
/// * **Reading:** you may read the slice as long as no other thread is
///   concurrently writing to the same elements (the Slayer Core only reads
///   after a write has been flushed, so a read‑only view is safe after the
///   application has performed the write).  
/// * **Writing:** you must have exclusive access to the underlying region;
///   typically you would use `UltraSlayer::write` instead of mutating the slice
///   directly.
///
/// The type implements `Deref`/`DerefMut` so you can treat it like a normal
/// slice when you are inside an `unsafe` block.
pub struct Slice<'a, T> {
    /// Pointer to the first element of the view.
    ptr: *mut T,
    /// Number of elements in the view.
    len: usize,
    /// Marker to tie the lifetime of the view to the owning slab.
    _marker: PhantomData<&'a mut T>,
}

impl<'a, T> Slice<'a, T> {
    /// Construct a new `Slice` from a raw pointer + length.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    ///   * `ptr` points to a valid `len`‑element region of memory that lives at
    ///     least as long as `'a`.
    ///   * The region is correctly aligned for `T`.
    ///   * No other mutable reference to the same memory exists while a
    ///     `Slice` (or any of its derived slice references) is used.
    #[inline]
    pub unsafe fn from_raw_parts(ptr: *mut T, len: usize) -> Self {
        Slice {
            ptr,
            len,
            _marker: PhantomData,
        }
    }

    /// Raw pointer to the first element (may be null if `len == 0`).
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.ptr as *const T
    }

    /// Raw mutable pointer to the first element (may be null if `len == 0`).
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr
    }

    /// Number of elements in the slice.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` if the view contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Unsafe read‑only view as a normal Rust slice.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the memory region is **not being
    /// mutated** concurrently.  In practice this means you should call
    /// `as_slice()` only after you have finished writing to the slab (or you
    /// are only reading data that the core has already flushed).
    #[inline]
    pub unsafe fn as_slice(&self) -> &[T] {
        std::slice::from_raw_parts(self.as_ptr(), self.len)
    }

    /// Unsafe mutable view as a normal Rust mutable slice.
    ///
    /// # Safety
    ///
    /// The caller must guarantee **exclusive** access to the underlying memory
    /// for the whole lifetime of the returned `&mut [T]`.  Using this while the
    /// Slayer Core or another thread is reading/writing the same region will
    /// break the aliasing contract and cause undefined behaviour.
    #[inline]
    pub unsafe fn as_mut_slice(&mut self) -> &mut [T] {
        std::slice::from_raw_parts_mut(self.as_mut_ptr(), self.len)
    }
}

impl<'a, T> Deref for Slice<'a, T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        unsafe { self.as_slice() }
    }
}

impl<'a, T> DerefMut for Slice<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { self.as_mut_slice() }
    }
}

impl<'a, T: std::fmt::Debug> std::fmt::Debug for Slice<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe { f.debug_struct("Slice")
                  .field("ptr", &self.as_ptr())
                  .field("len", &self.len)
                  .field("data", &self.as_slice())
                  .finish() }
    }
}

impl<'a, T: PartialEq> PartialEq for Slice<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        unsafe { self.as_slice() == other.as_slice() }
    }
}

impl<'a, T: Eq> Eq for Slice<'a, T> {}

