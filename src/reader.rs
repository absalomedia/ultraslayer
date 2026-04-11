use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam::utils::CachePadded;

use crate::arch::ArchConfig;
use crate::slab::HugeSlab;

// ---------------------------------------------------------------------------
// Signalling protocol
// ---------------------------------------------------------------------------
const STATE_IDLE: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_DONE: u8 = 2;

pub const MAX_T_SIZE: usize = 64;
const RESULT_SLOT_BACK_OFFSET: usize = MAX_T_SIZE * 2;

pub struct UltraSlayer<T> {
    slab: Arc<HugeSlab>,
    request_idx: Arc<CachePadded<AtomicUsize>>,
    state: Arc<CachePadded<AtomicU8>>,
    result_slot_offset: usize,
    num_replicas: usize,
    config: ArchConfig,
    elem_size: usize,
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
        }
    }

    pub fn insert(&self, index: usize, value: T) {
        let size = self.elem_size;
        let slab_size = self.slab.size();

        for r in 0..self.num_replicas {
            let offset = self.config.replica_offset.checked_mul(r)
                .and_then(|o| o.checked_add(index.checked_mul(size).expect("index overflow")))
                .expect("replica offset overflow");

            assert!(offset.checked_add(size).map_or(false, |end| end <= slab_size), "insert out of bounds");

            unsafe {
                let dst = self.slab.ptr().add(offset);
                std::ptr::copy_nonoverlapping(&value as *const T as *const u8, dst, size);
                std::sync::atomic::fence(Ordering::Release);
            }
        }
    }

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
                let core_ids = core_affinity::get_core_ids().expect("could not enumerate cores");
                let target = core_ids.into_iter().find(|c| c.id == cpu_id)
                    .unwrap_or_else(|| panic!("cpu_id {cpu_id} not found"));
                core_affinity::set_for_current(target);

                loop {
                    // 1. ULTRA-LOW LATENCY WAIT
                    // We replace Backoff::snooze with platform-specific "Pause" instructions.
                    // This keeps the core warm and prevents C-state transitions without
                    // triggering OS context switches.
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

                        // 2. EXPLICIT HARDWARE PREFETCHING
                        // Trigger the DRAM channels BEFORE the volatile read.
                        // This maximizes the chance that the "winning" channel is already in L1.
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

                        // Torn write detection
                        let agreed = (0..words).all(|w| replica_bufs[0][w] == replica_bufs[1][w]);
                        let src_words = if agreed { &replica_bufs[0] } else { &[0u64; 8] };

                        let result_dst = base.add(result_slot_offset) as *mut u64;
                        for w in 0..words {
                            std::ptr::write_volatile(result_dst.add(w), src_words[w]);
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

        // Pure spin-loop for the read path. 
        // No snooze/yield here; we want the result the instant the Slayer Core writes it.
        while self.state.load(Ordering::SeqCst) != STATE_DONE {
            std::hint::spin_loop();
            #[cfg(target_arch = "x86_64")]
            unsafe { std::arch::x86_64::_mm_pause(); }
            #[cfg(target_arch = "aarch64")]
            unsafe { std::arch::asm!("yield"); }
        }

        let result: T = unsafe {
            let src = self.slab.ptr().add(self.result_slot_offset) as *const T;
            std::ptr::read_volatile(src)
        };

        self.state.store(STATE_IDLE, Ordering::SeqCst);
        result
    }
}
