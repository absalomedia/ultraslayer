use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use crossbeam::utils::CachePadded;

use crate::arch::ArchConfig;
use crate::slab::HugeSlab;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinPolicy {
    Busy = 0,
    HybridYield = 1,
    Sleep = 2,
}

pub struct SlayerStats {
    pub total_reads: u64,
    pub wins: u64,
    pub misses: u64,
}

const STATE_IDLE: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_DONE: u8 = 2;

pub const MAX_T_SIZE: usize = 64;
const RESULT_SLOT_BACK_OFFSET: usize = MAX_T_SIZE * 2;

struct SeqLock {
    seq: AtomicUsize,
}

impl SeqLock {
    fn new() -> Self { Self { seq: AtomicUsize::new(0) } }
    #[inline(always)] fn write_begin(&self) { self.seq.fetch_add(1, Ordering::Release); }
    #[inline(always)] fn write_end(&self) { self.seq.fetch_add(1, Ordering::Release); }
    #[inline(always)] fn read_begin(&self) -> usize { self.seq.load(Ordering::Acquire) }
    #[inline(always)] fn read_end(&self, start: usize) -> bool {
        (start & 1 == 0) && (self.seq.load(Ordering::Acquire) == start)
    }
}

pub struct UltraSlayer<T> {
    slab: Arc<HugeSlab>,
    request_idx: Arc<CachePadded<AtomicUsize>>,
    state: Arc<CachePadded<AtomicU8>>,
    spin_policy: Arc<AtomicU8>,
    result_slot_offset: usize,
    num_replicas: usize,
    config: ArchConfig,
    elem_size: usize,
    seqlock: Arc<SeqLock>,
    #[allow(dead_code)]
    capacity: usize,
    _marker: PhantomData<T>,
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
        assert!(config.replica_offset >= capacity * elem_size, "replica_offset too small");

        let data_size = config.replica_offset * num_replicas;
        let slab_size = data_size.checked_add(RESULT_SLOT_BACK_OFFSET).expect("slab size overflow");

        Self {
            slab: Arc::new(HugeSlab::new(slab_size)),
            request_idx: Arc::new(CachePadded::new(AtomicUsize::new(0))),
            state: Arc::new(CachePadded::new(AtomicU8::new(STATE_IDLE))),
            spin_policy: Arc::new(AtomicU8::new(SpinPolicy::Busy as u8)),
            result_slot_offset: data_size,
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

    #[cfg(feature = "slice")]
    pub unsafe fn slice(&self) -> crate::slice::Slice<'_, T> {
        crate::slice::Slice::from_raw_parts(self.slab.ptr() as *mut T, self.capacity)
    }

    pub fn set_spin_policy(&self, policy: SpinPolicy) {
        self.spin_policy.store(policy as u8, Ordering::SeqCst);
    }

    pub fn stats(&self) -> SlayerStats {
        SlayerStats {
            total_reads: self.total_reads.load(Ordering::Relaxed) as u64,
            wins: self.wins.load(Ordering::Relaxed) as u64,
            misses: self.misses.load(Ordering::Relaxed) as u64,
        }
    }

    pub fn pin_to_core(&self, core_id: usize) {
        let core_ids = core_affinity::get_core_ids().expect("core_affinity failed");
        let target = core_ids.into_iter().find(|c| c.id == core_id).unwrap();
        core_affinity::set_for_current(target);
    }

    pub fn insert(&self, index: usize, value: T) {
        let size = self.elem_size;
        let slab_size = self.slab.size();
        self.seqlock.write_begin();
        for r in 0..self.num_replicas {
            let offset = self.config.replica_offset * r + index * size;
            assert!(offset + size <= slab_size, "insert out of bounds");
            unsafe { std::ptr::copy_nonoverlapping(&value as *const T as *const u8, self.slab.ptr().add(offset), size); }
        }
        self.seqlock.write_end();
        std::sync::atomic::fence(Ordering::Release);
    }

    pub fn spawn_slayer_core(&self, cpu_id: usize) {
        let (request_idx, state, spin_policy, slab, replica_offset, result_slot_offset, size, num_replicas, seqlock, total_reads, wins, misses) = 
            (Arc::clone(&self.request_idx), Arc::clone(&self.state), Arc::clone(&self.spin_policy), Arc::clone(&self.slab), self.config.replica_offset, self.result_slot_offset, self.elem_size, self.num_replicas, Arc::clone(&self.seqlock), Arc::clone(&self.total_reads), Arc::clone(&self.wins), Arc::clone(&self.misses));

        std::thread::Builder::new().name(format!("slayer-core-{cpu_id}")).spawn(move || {
            let core_ids = core_affinity::get_core_ids().unwrap();
            core_affinity::set_for_current(core_ids.into_iter().find(|c| c.id == cpu_id).unwrap());
            let slab_ptr = slab.ptr();
            let slab_size = slab.size();
            unsafe { for i in (0..slab_size).step_by(4096) { std::ptr::write_volatile(slab_ptr.add(i), 0); } }

            loop {
                while state.load(Ordering::SeqCst) != STATE_READY {
                    let policy = spin_policy.load(Ordering::Relaxed);
                    match policy {
                        0 => { 
                            std::hint::spin_loop();
                            #[cfg(target_arch = "x86_64")] { std::arch::x86_64::_mm_pause(); }
                            #[cfg(target_arch = "aarch64")] { std::arch::asm!("yield"); }
                        },
                        1 => { for _ in 0..100 { std::hint::spin_loop(); } std::thread::yield_now(); },
                        _ => { std::thread::sleep(std::time::Duration::from_micros(10)); }
                    }
                }
                let idx = request_idx.load(Ordering::SeqCst);
                total_reads.fetch_add(1, Ordering::Relaxed);
                let max_offset = replica_offset * (num_replicas - 1) + idx * size + size;
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
                            #[cfg(target_arch = "x86_64")] std::arch::x86_64::_mm_prefetch(addr as *const i8, std::arch::x86_64::_MM_HINT_T0);
                        }
                        let mut replica_bufs: [[u64; 8]; 2] = [[0u64; 8]; 2];
                        for r in 0..num_replicas.min(2) {
                            let addr = base.add(replica_offset * r + idx * size) as *const u64;
                            for w in 0..words { replica_bufs[r][w] = std::ptr::read_volatile(addr.add(w)); }
                        }
                        if seqlock.read_end(seq_start) {
                            let result_dst = base.add(result_slot_offset) as *mut u64;
                            for w in 0..words { std::ptr::write_volatile(result_dst.add(w), replica_bufs[0][w]); }
                            wins.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                state.store(STATE_DONE, Ordering::SeqCst);
            }
        }).unwrap();
    }

    #[inline(always)]
    pub fn read(&self, index: usize) -> T {
        self.request_idx.store(index, Ordering::SeqCst);
        self.state.store(STATE_READY, Ordering::SeqCst);
        while self.state.load(Ordering::SeqCst) != STATE_DONE {
            std::hint::spin_loop();
            #[cfg(target_arch = "x86_64")] unsafe { std::arch::x86_64::_mm_pause(); }
            #[cfg(target_arch = "aarch64")] unsafe { std::arch::asm!("yield"); }
        }
        let result: T = unsafe { std::ptr::read_volatile(self.slab.ptr().add(self.result_slot_offset) as *const T) };
        self.state.store(STATE_IDLE, Ordering::SeqCst);
        result
    }
}
