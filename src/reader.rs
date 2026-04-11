use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use crossbeam::utils::CachePadded;

use crate::arch::ArchConfig;
use crate::slab::HugeSlab;

// ---------------------------------------------------------------------------
// Signalling protocol
// ---------------------------------------------------------------------------
// SPSC Handshake: IDLE (0) -> READY (1) -> DONE (2) -> IDLE (0)
const STATE_IDLE: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_DONE: u8 = 2;

/// Maximum supported size of T in bytes.
pub const MAX_T_SIZE: usize = 64;

/// Byte offset from the end of the usable slab for the result slot.
const RESULT_SLOT_BACK_OFFSET: usize = MAX_T_SIZE * 2;

// ---------------------------------------------------------------------------
// Sequence Lock (Seqlock)
// ---------------------------------------------------------------------------
/// A Seqlock prevents "torn reads" where the reader sees a value half-updated by the writer.
/// Writer: Increment seq (make odd) -> Write Data -> Increment seq (make even).
/// Reader: Read seq -> Read Data -> Read seq. If seq is odd or changed, retry.
struct SeqLock {
    seq: AtomicUsize,
}

impl SeqLock {
    fn new() -> Self {
        Self { seq: AtomicUsize::new(0) }
    }

    #[inline(always)]
    fn write_begin(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }

    #[inline(always)]
    fn write_end(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }

    #[inline(always)]
    fn read_begin(&self) -> usize {
        self.seq.load(Ordering::Acquire)
    }

    #[inline(always)]
    fn read_end(&self, start: usize) -> bool {
        // Success if the sequence is even AND hasn't changed since we started reading.
        (start & 1 == 0) && (self.seq.load(Ordering::Acquire) == start)
    }
}

// ---------------------------------------------------------------------------
// UltraSlayer
// ---------------------------------------------------------------------------

pub struct UltraSlayer<T> {
    slab: Arc<HugeSlab>,
    /// Index of the requested element, written by caller before READY.
    request_idx: Arc<CachePadded<AtomicUsize>>,
    /// Handshake byte: IDLE -> READY -> DONE -> IDLE.
    state: Arc<CachePadded<AtomicU8>>,
    /// Byte offset into the slab at which the result slot lives.
    result_slot_offset: usize,
    /// Number of replicas written by `insert` and read by the slayer core.
    num_replicas: usize,
    config: ArchConfig,
    /// Byte stride of a single element.
    elem_size: usize,
    /// Lock to prevent torn reads.
    seqlock: Arc<SeqLock>,
}

impl<T: Copy + Send + Sync + 'static> UltraSlayer<T> {
    pub fn new(num_replicas: usize, capacity: usize) -> Self {
        assert!(num_replicas >= 2, "num_replicas must be >= 2 for hedging");
        let elem_size = std::mem::size_of::<T>();
        assert!(elem_size <= MAX_T_SIZE, "T is {elem_size} bytes; MAX_T_SIZE is {MAX_T_SIZE}");

        let config = ArchConfig::for_platform();

        assert!(
            config.replica_offset >= capacity * elem_size,
            "replica_offset ({}) < capacity * elem_size ({}).",
            config.replica_offset, capacity * elem_size
        );

        let data_size = config.replica_offset * num_replicas;
        let slab_size = data_size.checked_add(RESULT_SLOT_BACK_OFFSET).expect("slab size overflow");

        let slab = Arc::new(HugeSlab::new(slab_size));
        let result_slot_offset = data_size;

        Self {
            slab,
            request_idx: Arc::new(CachePadded::new(AtomicUsize::new(0))),
            state: Arc::new(CachePadded::new(AtomicU8::new(STATE_IDLE))),
            result_slot_offset,
            num_replicas,
            config,
            elem_size,
            seqlock: Arc::new(SeqLock::new()),
        }
    }

    /// Writes `value` to every replica at position `index`.
    pub fn insert(&self, index: usize, value: T) {
        let size = self.elem_size;
        let slab_size = self.slab.size();

        // Begin Seqlock: Marks the start of a write (seq becomes odd).
        self.seqlock.write_begin();

        for r in 0..self.num_replicas {
            let offset = self.config.replica_offset.checked_mul(r)
                .and_then(|o| o.checked_add(index.checked_mul(size).expect("index overflow")))
                .expect("replica offset overflow");

            assert!(offset.checked_add(size).map_or(false, |end| end <= slab_size), "insert out of bounds");

            unsafe {
                let dst = self.slab.ptr().add(offset);
                std::ptr::copy_nonoverlapping(&value as *const T as *const u8, dst, size);
            }
        }
        
        // End Seqlock: Marks the completion of the write (seq becomes even).
        self.seqlock.write_end();
        std::sync::atomic::fence(Ordering::Release);
    }

    pub fn spawn_slayer_core(&self, cpu_id: usize) {
        let request_idx = Arc::clone(&self.request_idx);
        let state = Arc::clone(&self.state);
        let slab = Arc::clone(&self.slab);
        let replica_offset = self.config.replica_offset;
        let result_slot_offset = self.result_slot_offset;
        let size = self.elem_size;
        let num_replicas = self.num_replicas;
        let seqlock = Arc::clone(&self.seqlock);

        std::thread::Builder::new()
            .name(format!("slayer-core-{cpu_id}"))
            .spawn(move || {
                let core_ids = core_affinity::get_core_ids().expect("core_affinity failed");
                let target = core_ids.into_iter().find(|c| c.id == cpu_id)
                    .unwrap_or_else(|| panic!("cpu_id {cpu_id} not found"));
                core_affinity::set_for_current(target);

                unsafe {
                    // FIRST-TOUCH: Force the OS to allocate the physical pages on THIS NUMA node.
                    // We write zeros to every page in the slab from the Slayer Core.
                    for i in (0..slab_size).step_by(4096) {
                        std::ptr::write_volatile(slab_ptr.add(i), 0);
                            }
                    }

                loop {
                    // 1. WARM-SPIN: Wait for READY signal.
                    while state.load(Ordering::SeqCst) != STATE_READY {
                        std::hint::spin_loop();
                        #[cfg(target_arch = "x86_64")]
                        unsafe { std::arch::x86_64::_mm_pause(); }
                        #[cfg(target_arch = "aarch64")]
                        unsafe { std::arch::asm!("yield"); }
                    }

                    let idx = request_idx.load(Ordering::SeqCst);

                    let max_offset = replica_offset
                        .saturating_mul(num_replicas - 1)
                        .saturating_add(idx.saturating_mul(size))
                        .saturating_add(size);
                    
                    if max_offset > result_slot_offset {
                        state.store(STATE_DONE, Ordering::SeqCst);
                        continue;
                    }

                    unsafe {
                        let base = slab.ptr();
                        let words = size.div_ceil(8).min(8);

                        // SEQLOCK RETRY LOOP: Ensure the read is atomic and not torn.
                        loop {
                            let seq_start = seqlock.read_begin();

                            // 2. SYMMETRIC HARDWARE PREFETCH
                            // We trigger the memory controller to fetch all replicas simultaneously.
                            for r in 0..num_replicas.min(2) {
                                let addr = base.add(replica_offset * r + idx * size);
                                #[cfg(target_arch = "x86_64")]
                                std::arch::x86_64::_mm_prefetch(addr as *const i8, std::arch::x86_64::_MM_HINT_T0);
                                #[cfg(target_arch = "aarch64")]
                                std::arch::asm!("prfm pldl1, [{0}]", in(reg) addr);
                            }

                            // 3. HEDGED READ
                            let mut replica_bufs: [[u64; 8]; 2] = [[0u64; 8]; 2];
                            for r in 0..num_replicas.min(2) {
                                let addr = base.add(replica_offset * r + idx * size) as *const u64;
                                for w in 0..words {
                                    replica_bufs[r][w] = std::ptr::read_volatile(addr.add(w));
                                }
                            }

                            // 4. VALIDATION & SEQLOCK CHECK
                            // If the seqlock hasn't changed, we have a consistent read.
                            if seqlock.read_end(seq_start) {
                                // Use replica 0 as the result.
                                let result_dst = base.add(result_slot_offset) as *mut u64;
                                for w in 0..words {
                                    std::ptr::write_volatile(result_dst.add(w), replica_bufs[0][w]);
                                }
                                break; // Success! Exit retry loop.
                            }
                            // If we reach here, a write occurred during our read. Retry immediately.
                        }
                    }

                    state.store(STATE_DONE, Ordering::SeqCst);
                }
            })
            .expect("failed to spawn slayer core thread");
    }

    #[inline(always)]
    pub fn read(&self, index: usize) -> T {
        // Publish index and signal READY.
        self.request_idx.store(index, Ordering::SeqCst);
        self.state.store(STATE_READY, Ordering::SeqCst);

        // Pure spin-wait for DONE.
        while self.state.load(Ordering::SeqCst) != STATE_DONE {
            std::hint::spin_loop();
            #[cfg(target_arch = "x86_64")]
            unsafe { std::arch::x86_64::_mm_pause(); }
            #[cfg(target_arch = "aarch64")]
            unsafe { std::arch::asm!("yield"); }
        }

        // Read the result bytes from the result slot in the slab.
        let result: T = unsafe {
            let src = self.slab.ptr().add(self.result_slot_offset) as *const T;
            std::ptr::read_volatile(src)
        };

        // Return core to IDLE.
        self.state.store(STATE_IDLE, Ordering::SeqCst);

        result
    }
}
