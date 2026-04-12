use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use crossbeam::utils::CachePadded;

use crate::arch::ArchConfig;
use crate::slab::HugeSlab;

// ---------------------------------------------------------------------------
// Spin Policy & Stats
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinPolicy {
    /// 100% CPU spin. Lowest latency, highest power.
    Busy = 0,
    /// Spin with occasional yield to OS. Balanced.
    HybridYield = 1,
    /// Periodic sleep. Lowest power, higher latency.
    Sleep = 2,
}

pub struct SlayerStats {
    pub total_reads: u64,
    pub wins: u64,
    pub misses: u64,
}

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
    spin_policy: Arc<AtomicU8>, // Store policy as Atomic for the core thread
    /// Byte offset into the slab at which the result slot lives.
    result_slot_offset: usize,
    /// Number of replicas written by `insert` and read by the slayer core.
    num_replicas: usize,
    config: ArchConfig,
    /// Byte stride of a single element.
    elem_size: usize,
    /// Lock to prevent torn reads.
    seqlock: Arc<SeqLock>,
    /// The maximum number of elements this slab can hold.
    #[allow(dead_code)]
    capacity: usize,
    /// Marker to tell Rust that UltraSlayer "owns" T, even though it uses raw pointers.
    _marker: PhantomData<T>,
    
    // Stats counters
    total_reads: Arc<AtomicUsize>,
    wins: Arc<AtomicUsize>,
    misses: Arc<AtomicUsize>,
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
            spin_policy: Arc::new(AtomicU8::new(SpinPolicy::Busy as u8)),
            result_slot_offset,
            num_replicas,
            config,
            elem_size,
            seqlock: Arc::new(SeqLock::new()),
            capacity,
            _marker: PhantomData,
            total_reads: Arc::new(AtomicUsize::new(0)),
            wins: Arc::new(AtomicUsize::new(0)),
            misses: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a zero-copy view of the first DRAM replica.
    #[cfg(feature = "slice")]
    pub unsafe fn slice(&self) -> crate::slice::Slice<'_, T> {
        crate::slice::Slice::from_raw_parts(self.slab.ptr() as *mut T, self.capacity)
    }

    /// Updates the spin policy for the Slayer Core in real-time.
    pub fn set_spin_policy(&self, policy: SpinPolicy) {
        self.spin_policy.store(policy as u8, Ordering::SeqCst);
    }

    /// Returns current operational statistics.
    pub fn stats(&self) -> SlayerStats {
        SlayerStats {
            total_reads: self.total_reads.load(Ordering::Relaxed) as u64,
            wins: self.wins.load(Ordering::Relaxed) as u64,
            misses: self.misses.load(Ordering::Relaxed) as u64,
        }
    }

    pub fn pin_to_core(&self, core_id: usize) {
        let core_ids = core_affinity::get_core_ids().expect("core_affinity failed");
        let target = core_ids.into_iter().find(|c| c.id == core_id)
            .unwrap_or_else(|| panic!("cpu_id {core_id} not found"));
        core_affinity::set_for_current(target);
    }

    pub fn insert(&self, index: usize, value: T) {
        let size = self.elem_size;
        let slab_size = self.slab.size();

        // 1. Begin Seqlock: Marks the start of a write (sequence becomes odd).
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
        
        // -----------------------------------------------------------------
        // MEMORY BARRIER: Ensure mirrored writes are globally visible BEFORE
        // we close the sequence lock. Prevents the Slayer Core from reading
        // inconsistent data (Fix from TailSlayer PR #9).
        // -----------------------------------------------------------------
        std::sync::atomic::fence(Ordering::Release);

        // 2. End Seqlock: Marks the completion of the write (sequence becomes even).
        self.seqlock.write_end();
    }

    pub fn spawn_slayer_core(&self, cpu_id: usize) {
        let request_idx = Arc::clone(&self.request_idx);
        let state = Arc::clone(&self.state);
        let spin_policy = Arc::clone(&self.spin_policy);
        let slab = Arc::clone(&self.slab);
        let replica_offset = self.config.replica_offset;
        let result_slot_offset = self.result_slot_offset;
        let size = self.elem_size;
        let num_replicas = self.num_replicas;
        let seqlock = Arc::clone(&self.seqlock);
        let total_reads = Arc::clone(&self.total_reads);
        let wins = Arc::clone(&self.wins);
        let misses = Arc::clone(&self.misses);

        std::thread::Builder::new()
            .name(format!("slayer-core-{cpu_id}"))
            .spawn(move || {
                let core_ids = core_affinity::get_core_ids().expect("core_affinity failed");
                let target = core_ids.into_iter().find(|c| c.id == cpu_id)
                    .unwrap_or_else(|| panic!("cpu_id {cpu_id} not found"));
                core_affinity::set_for_current(target);

                let slab_ptr = slab.ptr();
                let slab_size = slab.size();

                unsafe {
                    for i in (0..slab_size).step_by(4096) {
                        std::ptr::write_volatile(slab_ptr.add(i), 0);
                    }
                }

                loop {
                    while state.load(Ordering::SeqCst) != STATE_READY {
                        let policy = spin_policy.load(Ordering::Relaxed);
                        match policy {
                            0 => { // Busy
                                std::hint::spin_loop();
                                #[cfg(target_arch = "x86_64")]
                                { std::arch::x86_64::_mm_pause(); }
                                #[cfg(target_arch = "aarch64")]
                                { std::arch::asm!("yield"); }
                            },
                            1 => { // HybridYield
                                for _ in 0..100 { std::hint::spin_loop(); }
                                std::thread::yield_now();
                            },
                            _ => { // Sleep
                                std::thread::sleep(std::time::Duration::from_micros(10));
                            }
                        }
                    }

                    let idx = request_idx.load(Ordering::SeqCst);
                    total_reads.fetch_add(1, Ordering::Relaxed);

                    let max_offset = replica_offset
                        .saturating_mul(num_replicas - 1)
                        .saturating_add(idx.saturating_mul(size))
                        .saturating_add(size);
                    
                    if max_offset > result_slot_offset {
                        state.store(STATE_DONE, Ordering::SeqCst);
                        misses.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }

                    unsafe {
                        let base = slab.ptr();
                        let words = size.div_ceil(8).min(8);

                        loop {
                            let seq_start = seqlock.read_begin();

                            for r in 0..num_replicas.min(2) {
                                let addr = base.add(replica_offset * r + idx * size);
                                #[cfg(target_arch = "x86_64")]
                                std::arch::x86_64::_mm_prefetch(addr as *const i8, std::arch::x86_64::_MM_HINT_T0);
                                #[cfg(target_arch = "aarch64")]
                                std::arch::asm!("prfm pldl1, [{0}]", in(reg) addr);
                            }

                            let mut replica_bufs: [[u64; 8]; 2] = [[0u64; 8]; 2];
                            for r in 0..num_replicas.min(2) {
                                let addr = base.add(replica_offset * r + idx * size) as *const u64;
                                for w in 0..words {
                                    replica_bufs[r][w] = std::ptr::read_volatile(addr.add(w));
                                }
                            }

                            if seqlock.read_end(seq_start) {
                                let result_dst = base.add(result_slot_offset) as *mut u64;
                                for w in 0..words {
                                    std::ptr::write_volatile(result_dst.add(w), replica_bufs[0][w]);
                                }
                                wins.fetch_add(1, Ordering::Relaxed);
                                break;
                            }
                        }
                    }

                    state.store(STATE_DONE, Ordering::SeqCst);
                }
            })
            .expect("failed to spawn slayer core thread");
    }

    #[inline(always)]
    pub fn read(&self, index: usize) -> T {
        self.request_idx.store(index, Ordering::SeqCst);
        self.state.store(STATE_READY, Ordering::SeqCst);

        while self.state.load(Ordering::SeqCst) != STATE_DONE {
            std::hint::spin_loop();
            #[cfg(target_arch = "x86_64")]
            { std::arch::x86_64::_mm_pause(); }
            #[cfg(target_arch = "aarch64")]
            { std::arch::asm!("yield"); }
        }

        let result: T = unsafe {
            let src = self.slab.ptr().add(self.result_slot_offset) as *const T;
            std::ptr::read_volatile(src)
        };

        self.state.store(STATE_IDLE, Ordering::SeqCst);
        result
    }
}
