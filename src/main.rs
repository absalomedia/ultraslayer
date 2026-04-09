use ultraslayer_rs::UltraSlayer;
use std::time::Instant;

fn main() {
    println!("⚡️ Initializing UltraSlayer (Hedged DRAM Read / Ultra-Low Latency)...");

    // 2 replicas (DRAM channels), capacity for 1 million u64 elements.
    // Each replica is separated by ArchConfig::replica_offset (64 MiB) to
    // guarantee placement on a different DRAM channel.
    let capacity = 1_000_000usize;
    let slayer = UltraSlayer::<u64>::new(2, capacity);

    // Write a critical value (e.g. top-of-book price) to both replicas.
    slayer.insert(0, 123_456_789u64);

    // Pin the slayer core to CPU 1 (not 0, which handles OS interrupts).
    // In production, use an isolcpus-isolated core.
    println!("Spawning Slayer Core on CPU 1...");
    slayer.spawn_slayer_core(1);

    // Allow the core to reach its spin loop before measuring.
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Warm up: fill L1/L2 caches and stabilise branch predictors.
    for _ in 0..10_000 {
        let _ = slayer.read(0);
    }

    // Single cold-read latency measurement.
    let start = Instant::now();
    let val = slayer.read(0);
    let duration = start.elapsed();

    println!("Slayer Read Result : {val}");
    println!("Single Read Latency: {duration:?}");

    // Median over 1000 samples for a more stable latency figure.
    let mut samples: Vec<u64> = (0..1000)
        .map(|_| {
            let t = Instant::now();
            let _ = slayer.read(0);
            t.elapsed().as_nanos() as u64
        })
        .collect();
    samples.sort_unstable();
    println!(
        "p50 latency: {}ns  p99: {}ns  p100: {}ns",
        samples[500],
        samples[990],
        samples[999]
    );
}
