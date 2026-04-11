//! UltraSlayer – a DRAM‑refresh‑stall‑immune memory slab.
//!
//! The public API is intentionally small:
//!   * `UltraSlayer<T>` – the high‑performance slab itself.
//!   * `HugeSlab<T>`    – the low‑level backing allocator.
//!   * `ArchConfig`     – CPU‑affinity / NUMA helpers.
//!   * `SpinPolicy`    – runtime spin‑policy selector.
//!   * `ShmSlab<T>`    – POSIX shared‑memory wrapper (optional).
//!   * `Slice<T>`      – zero‑copy view into a slab.
//!   * (optional) C‑FFI side‑car when the `sidecar` feature is enabled.
//!
//! The library is deliberately `no_std`‑compatible *inside* the core, but
//! the public wrapper uses the standard library for convenience.

/// Architecture‑specific helpers (CPU pinning, NUMA, etc.).
pub mod arch;
/// Low‑level memory‑allocation and mirroring logic.
pub mod slab;
/// The high‑level slab type (`UltraSlayer<T>`) and its public methods.
pub mod reader;

/// Re‑export the most‑used items so downstream crates can write
/// `use ultraslayer::{UltraSlayer, HugeSlab, ArchConfig, SpinPolicy};`
pub use arch::ArchConfig;
pub use reader::{SpinPolicy, UltraSlayer};
pub use slab::HugeSlab;

/// Optional zero‑copy slice view – always available.
pub mod slice;
pub use slice::Slice;

/// Optional shared‑memory wrapper (POSIX `shm_open` + `mmap`).
#[cfg(feature = "shm")]
pub mod shm;

#[cfg(feature = "shm")]
pub use shm::ShmSlab;

/// Optional C‑FFI side‑car.  It is compiled only when the `sidecar` feature is
/// enabled, so crates that don’t need a C API won’t pull in the extra build
/// dependencies.
#[cfg(feature = "sidecar")]
pub mod ffi;

#[cfg(feature = "sidecar")]
pub use ffi::*;
