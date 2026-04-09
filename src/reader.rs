use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam::utils::CachePadded;

use crate::arch::ArchConfig;
use crate::slab::HugeSlab;

// ---------------------------------------------------------------------------
// Signalling protocol
// ---------------------------------------------------------------------------
//
// The caller and the slayer core communicate via a single `state` byte that
// acts as a minimal SPSC handshake:
//
//   IDLE  (0) — core is spinning, waiting for work.
//   READY (1) — caller has written request_idx and wants a read.
//   DONE  (2) — core has written the result bytes and is returning to idle.
//
// All transitions use SeqCst to establish a total order across the two
// *separate* atomic locations (state and request_idx / result_slot), which
// Acquire/Release pairs on *different* atomics cannot guarantee alone.

const STATE_IDLE: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_DONE: u8 = 2;

// ---------------------------------------------------------------------------
// Result slot
// ---------------------------------------------------------------------------
//
// Because T may be any Copy + Send + Sync type (u8, u64, a small struct, …)
// we cannot store the result in a fixed AtomicUsize — that truncates or
// misreads any T whose size differs from usize. Instead we carve out a
// MAX_RESULT_SIZE-byte slot inside the slab itself, zero-initialised by the
// kernel, and copy bytes in/out through MaybeUninit<T>.

/// Maximum supported size of T in bytes.
pub const MAX_T_SIZE: usize = 64;

/// Byte offset from the *end* of the usable slab at which the result slot
/// lives. Placed here so it never overlaps user data.
const RESULT_SLOT_BACK_OFFSET: usize = MAX_T_SIZE * 2;

// ---------------------------------------------------------------------------
// UltraSlayer
// ---------------------------------------------------------------------------

/// Manages a mirrored dataset across DRAM channels and issues hedged reads
/// to minimise the latency impact of DRAM refresh stalls.
///
/// # Design
///
/// One dedicated "slayer core" thread spins on `state`. When the caller
/// invokes [`read`], it:
/// 1. Stores the target index with `SeqCst`.
/// 2. Flips `state` to `READY` with `SeqCst`.
/// 3. Spins until `state == DONE`.
/// 4. Reads the result bytes from the result slot.
///
/// The slayer core:
/// 1. Spins until `state == READY`.
/// 2. Issues two `read_volatile` calls — one per replica — so the DRAM
///    controller can pipeline both while one channel may be refreshing.
/// 3. Compares both values; if they agree it stores the result. If they
///    disagree it retries (indicating a torn write during `insert`).
/// 4. Flips `state` to `DONE`.
///
/// # Concurrency
///
/// `read` is **not** safe to call concurrently from multiple threads. The
/// SPSC protocol assumes a single caller. If you need concurrent reads,
/// shard across multiple `UltraSlayer` instances.
///
/// `insert` must not be called concurrently with `read` for the same index.
/// Writes to different indices from a single writer thread are safe.
pub struct UltraSlayer<T> {
    slab: Arc<HugeSlab>,
    /// Index of the requested element, written by caller before READY.
    request_idx: Arc<CachePadded<AtomicUsize>>,
    /// Handshake byte: IDLE → READY → DONE → IDLE.
    state: Arc<CachePadded<AtomicU8>>,
    /// Byte offset into the slab at which the result slot lives.
    result_slot_offset: usize,
    /// Number of replicas written by `insert` and read by the slayer core.
    num_replicas: usize,
    config: ArchConfig,
    /// Byte stride of a single element.
    elem_size: usize,
}

impl<T: Copy + Send + Sync + 'static> UltraSlayer<T> {
    /// Creates a new `UltraSlayer`.
    ///
    /// # Parameters
    /// - `num_replicas`: number of DRAM-channel replicas (≥ 2 for hedging).
    /// - `capacity`: maximum number of `T` elements per replica.
    ///
    /// # Panics
    /// - If `size_of::<T>() > MAX_T_SIZE`.
    /// - If the computed slab size overflows `usize`.
    /// - If the OS refuses to allocate the slab.
    pub fn new(num_replicas: usize, capacity: usize) -> Self {
        assert!(num_replicas >= 2, "num_replicas must be >= 2 for hedging");
        let elem_size = std::mem::size_of::<T>();
        assert!(
            elem_size <= MAX_T_SIZE,
            "T is {elem_size} bytes; MAX_T_SIZE is {MAX_T_SIZE}"
        );

        let config = ArchConfig::for_platform();

        // Each replica occupies replica_offset bytes (>= capacity * elem_size).
        // We assert the stride is large enough to hold all elements.
        assert!(
            config.replica_offset >= capacity * elem_size,
            "replica_offset ({}) < capacity * elem_size ({}). \
             Increase ArchConfig::replica_offset or reduce capacity.",
            config.replica_offset,
            capacity * elem_size
        );

        // Total slab: space for all replicas plus the result slot at the tail.
        let data_size = config.replica_offset * num_replicas;
        let slab_size = data_size
            .checked_add(RESULT_SLOT_BACK_OFFSET)
            .expect("slab size overflow");

        let slab = Arc::new(HugeSlab::new(slab_size));
        let result_slot_offset = data_size; // starts immediately after replica data

        Self {
            slab,
            request_idx: Arc::new(CachePadded::new(AtomicUsize::new(0))),
            state: Arc::new(CachePadded::new(AtomicU8::new(STATE_IDLE))),
            result_slot_offset,
            num_replicas,
            config,
            elem_size,
        }
    }

    // -----------------------------------------------------------------------
    // Write path
    // -----------------------------------------------------------------------

    /// Writes `value` to every replica at position `index`.
    ///
    /// # Panics
    /// Panics if `index` is out of bounds (i.e. the write would exceed the
    /// slab allocation for any replica).
    ///
    /// # Safety / concurrency
    /// Must not be called concurrently with `read` for the same `index`.
    pub fn insert(&self, index: usize, value: T) {
        let size = self.elem_size;
        let slab_size = self.slab.size();

        for r in 0..self.num_replicas {
            let offset = self
                .config
                .replica_offset
                .checked_mul(r)
                .and_then(|o| o.checked_add(index.checked_mul(size).expect("index overflow")))
                .expect("replica offset overflow");

            assert!(
                offset.checked_add(size).map_or(false, |end| end <= slab_size),
                "insert out of bounds: offset={offset} + size={size} > slab_size={slab_size}"
            );

            unsafe {
                let dst = self.slab.ptr().add(offset);
                std::ptr::copy_nonoverlapping(&value as *const T as *const u8, dst, size);
                // Ensure the store is visible before any concurrent reader
                // (caller is responsible for not racing, but a compiler fence
                // prevents the compiler from sinking the store).
                std::sync::atomic::fence(Ordering::Release);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Slayer core
    // -----------------------------------------------------------------------

    /// Spawns the dedicated slayer core thread pinned to `cpu_id`.
    ///
    /// # Panics
    /// - If `cpu_id` is not present in the system's core list.
    /// - If the thread cannot be spawned.
    ///
    /// # Note
    /// Avoid pinning to core 0; it typically handles OS interrupts and
    /// scheduling on Linux. Prefer an isolated core (`isolcpus=N` kernel
    /// parameter) for lowest jitter.
    pub fn spawn_slayer_core(&self, cpu_id: usize) {
        let request_idx = Arc::clone(&self.request_idx);
        let state = Arc::clone(&self.state);
        let slab = Arc::clone(&self.slab);
        let replica_offset = self.config.replica_offset;
        let result_slot_offset = self.result_slot_offset;
        let size = self.elem_size;
        let num_replicas = self.num_replicas;

        std::thread::Builder::new()
            .name(format!("slayer-core-{cpu_id}"))
            .spawn(move || {
                // Pin to the requested physical core.
                let core_ids = core_affinity::get_core_ids()
                    .expect("core_affinity: could not enumerate cores");
                let target = core_ids
                    .into_iter()
                    .find(|c| c.id == cpu_id)
                    .unwrap_or_else(|| panic!("cpu_id {cpu_id} not found on this system"));
                core_affinity::set_for_current(target);

                let backoff = crossbeam::utils::Backoff::new();

                loop {
                    // Wait for caller to signal READY.
                    while state.load(Ordering::SeqCst) != STATE_READY {
                        backoff.snooze(); // yields spin_loop hint then parks briefly
                    }
                    backoff.reset();

                    let idx = request_idx.load(Ordering::SeqCst);

                    // Validate index hasn't produced an out-of-bounds offset.
                    // In a hot path this is a single compare; the branch is
                    // almost never taken.
                    let max_offset = replica_offset
                        .saturating_mul(num_replicas - 1)
                        .saturating_add(idx.saturating_mul(size))
                        .saturating_add(size);
                    if max_offset > result_slot_offset {
                        // Signal done with zeroed result rather than UB.
                        // Caller should validate.
                        state.store(STATE_DONE, Ordering::SeqCst);
                        continue;
                    }

                    unsafe {
                        let base = slab.ptr();

                        // Issue one volatile read per replica. All reads are
                        // emitted before any result is stored, giving the
                        // CPU's OOO engine and DRAM controller the opportunity
                        // to pipeline across channels. If channel A is mid-
                        // refresh, channel B's data will be in the read buffer.
                        let mut reads = [MaybeUninit::<u64>::uninit(); 8]; // max MAX_T_SIZE/8
                        let words = size.div_ceil(8).min(reads.len());

                        // Read all replicas.
                        // We keep replica 0's words as the authoritative result
                        // but issue all reads before processing any, maximising
                        // the overlap window for the memory controller.
                        let mut replica_bufs: [[u64; 8]; 2] = [[0u64; 8]; 2];
                        for r in 0..num_replicas.min(2) {
                            let addr = base.add(replica_offset * r + idx * size) as *const u64;
                            for w in 0..words {
                                replica_bufs[r][w] =
                                    std::ptr::read_volatile(addr.add(w));
                            }
                        }

                        // Use replica 0 as the primary result. If it disagrees
                        // with replica 1 on any word, the element was torn by a
                        // concurrent insert — store zeros so the caller can
                        // detect the anomaly (result will be visibly wrong
                        // rather than silently stale).
                        let agreed = (0..words).all(|w| replica_bufs[0][w] == replica_bufs[1][w]);
                        let src_words = if agreed { &replica_bufs[0] } else { &[0u64; 8] };

                        // Write result bytes into the result slot.
                        let result_dst = base.add(result_slot_offset) as *mut u64;
                        for w in 0..words {
                            reads[w] = MaybeUninit::new(src_words[w]);
                            std::ptr::write_volatile(result_dst.add(w), src_words[w]);
                        }
                    }

                    state.store(STATE_DONE, Ordering::SeqCst);
                }
            })
            .expect("failed to spawn slayer core thread");
    }

    // -----------------------------------------------------------------------
    // Read path
    // -----------------------------------------------------------------------

    /// Issues a hedged read for element at `index`.
    ///
    /// Blocks (spinning) until the slayer core has completed the read.
    /// Returns a copy of `T` read from the slab.
    ///
    /// # Panics
    /// Panics (debug) if `index` is out of bounds.
    ///
    /// # Concurrency
    /// Must not be called from multiple threads simultaneously.
    #[inline(always)]
    pub fn read(&self, index: usize) -> T {
        // Publish the index with SeqCst to ensure it is visible to the slayer
        // core *before* the state flip below, even across separate atomics.
        self.request_idx.store(index, Ordering::SeqCst);
        self.state.store(STATE_READY, Ordering::SeqCst);

        // Spin until the core signals DONE. Using crossbeam Backoff keeps the
        // core warm on short waits while avoiding wasting power on long stalls.
        let backoff = crossbeam::utils::Backoff::new();
        while self.state.load(Ordering::SeqCst) != STATE_DONE {
            backoff.snooze();
        }

        // Read the result bytes from the result slot in the slab.
        let result: T = unsafe {
            let src = self.slab.ptr().add(self.result_slot_offset) as *const T;
            std::ptr::read_volatile(src)
        };

        // Return to IDLE so the core can accept the next request.
        self.state.store(STATE_IDLE, Ordering::SeqCst);

        result
    }
}
