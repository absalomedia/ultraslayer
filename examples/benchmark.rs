use ultraslayer::UltraSlayer;
use std::time::{Instant, Duration};
use std::sync::Arc;
use std::thread;

fn main() {
    println!("🚀 Starting UltraSlayer Hardware Benchmark...");
    let capacity = 1_000_000usize;
    let slayer = Arc::new(UltraSlayer::<u64>::new(2, capacity));
    slayer.insert(0, 123_456_789u64);
    slayer.spawn_slayer_core(1);
    thread::sleep(Duration::from_millis(100));

    println!("\n[Test 1] Measuring Static Read Latency...");
    for _ in 0..10_000 { let _ = slayer.read(0); }
    let mut static_samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        let _ = slayer.read(0);
        static_samples.push(start.elapsed().as_nanos() as u64);
    }
    static_samples.sort_unstable();
    println!("Static Read -> p50: {}ns | p99: {}ns | p100: {}ns", 
        static_samples[500], static_samples[990], static_samples[999]);

    println!("\n[Test 2] Measuring Concurrent Update Latency (Torn-Read Test)...");
    let slayer_clone = Arc::clone(&slayer);
    let writer_handle = thread::spawn(move || {
        for i in 0..1_000_000 { slayer_clone.insert(0, i as u64); }
    });
    let mut dynamic_samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        let _ = slayer.read(0);
        dynamic_samples.push(start.elapsed().as_nanos() as u64);
    }
    writer_handle.join().unwrap();
    dynamic_samples.sort_unstable();
    println!("Dynamic Read -> p50: {}ns | p99: {}ns | p100: {}ns", 
        dynamic_samples[500], dynamic_samples[990], dynamic_samples[999]);
}
