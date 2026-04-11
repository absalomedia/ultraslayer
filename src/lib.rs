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
pub use reader::{SpinPolicy, UltraSlayer};
pub use slab::HugeSlab;
pub use arch::ArchConfig;

// Optional: Zero-copy slice view
#[cfg(feature = "slice")]
pub mod slice;
#[cfg(feature = "slice")]
pub use slice::Slice;

// Optional: Shared-memory wrapper (Linux only)
#[cfg(all(feature = "shm", unix))]
pub mod shm;
#[cfg(all(feature = "shm", unix))]
pub use shm::ShmSlab;

// Optional: C-FFI side-car
#[cfg(feature = "sidecar")]
pub mod ffi;
#[cfg(feature = "sidecar")]
#[allow(unused_imports)]
pub use ffi::*;
