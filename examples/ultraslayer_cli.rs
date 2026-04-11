//! examples/ultraslayer_cli.rs
//!
//! A tiny command‑line driver that mirrors the usage shown in the README.
//! It parses a few flags (`--channels`, `--size`, `--spin`), creates the
//! slab, starts the background core, and then just idles until the user
//! hits Ctrl‑C.

use std::process;
use std::thread;
use std::time::Duration;

use clap::{Arg, Command};

use ultraslayer::{UltraSlayer, SpinPolicy};

fn parse_size(s: &str) -> Result<usize, String> {
    // Accept suffixes like KiB, MiB, GiB, TiB (case‑insensitive).
    let s = s.trim().to_ascii_lowercase();
    if let Some(num) = s.strip_suffix("kib") {
        return num
            .parse::<usize>()
            .map(|v| v * 1024)
            .map_err(|e| e.to_string());
    }
    if let Some(num) = s.strip_suffix("mib") {
        return num
            .parse::<usize>()
            .map(|v| v * 1024 * 1024)
            .map_err(|e| e.to_string());
    }
    if let Some(num) = s.strip_suffix("gib") {
        return num
            .parse::<usize>()
            .map(|v| v * 1024 * 1024 * 1024)
            .map_err(|e| e.to_string());
    }
    if let Some(num) = s.strip_suffix("tib") {
        return num
            .parse::<usize>()
            .map(|v| v * 1024 * 1024 * 1024 * 1024)
            .map_err(|e| e.to_string());
    }
    // Fallback – plain number of bytes.
    s.parse::<usize>()
        .map_err(|e| format!("invalid size \"{}\": {}", s, e))
}

fn main() {
    // -------------------------------------------------------------
    // CLI parsing – updated for clap v4
    // -------------------------------------------------------------
    let matches = Command::new("ultraslayer")
        .about("Demo binary for the UltraSlayer memory slab")
        .arg(
            Arg::new("channels")
                .long("channels")
                .value_name("N")
                .help("Number of DRAM mirrors (default 2)")
                .default_value("2")
                .required(false),
        )
        .arg(
            Arg::new("size")
                .long("size")
                .value_name("SIZE")
                .help("Total slab size per channel (e.g. 2GiB, 512MiB) – required")
                .required(true),
        )
        .arg(
            Arg::new("spin")
                .long("spin")
                .value_name("POLICY")
                .help("Spin policy: busy | hybrid | sleep (default busy)")
                .default_value("busy")
                .required(false),
        )
        .get_matches();

    // In clap v4, value_of is replaced by get_one::<String>()
    let channels: usize = matches
        .get_one::<String>("channels")
        .unwrap()
        .parse()
        .expect("failed to parse channels");

    let size_str = matches.get_one::<String>("size").unwrap(); 
    let size_bytes = parse_size(size_str).unwrap_or_else(|e| {
        eprintln!("Error parsing --size: {}", e);
        process::exit(1);
    });

    let spin_policy = match matches.get_one::<String>("spin").unwrap().as_str() {
        "busy" => SpinPolicy::Busy,
        "hybrid" => SpinPolicy::HybridYield,
        "sleep" => SpinPolicy::Sleep,
        other => {
            eprintln!("Invalid spin policy \"{}\" – use busy|hybrid|sleep", other);
            process::exit(1);
        }
    };

    // -------------------------------------------------------------
    // Create the slab.
    // -------------------------------------------------------------
    // Fixed: Using .new() instead of .with_channels()
    let slayer = UltraSlayer::<u64>::new(channels, size_bytes);
    
    slayer.set_spin_policy(spin_policy);
    
    // The slayer core needs a CPU ID to pin to. 
    // We use core 0 by default for the demo.
    slayer.spawn_slayer_core(0);

    // -------------------------------------------------------------
    // Print a tiny status line so the user knows it is alive.
    // -------------------------------------------------------------
    println!(
        "UltraSlayer running – channels: {}, size per channel: {} bytes, spin: {:?}",
        channels,
        size_bytes,
        spin_policy
    );
    println!("Press Ctrl‑C to stop...");

    // -------------------------------------------------------------
    // Idle loop – keep the process alive while the core spins.
    // -------------------------------------------------------------
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
