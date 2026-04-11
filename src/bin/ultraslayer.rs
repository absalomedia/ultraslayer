//! src/bin/ultraslayer.rs
//!
//! A tiny command‑line driver that mirrors the usage shown in the README.
//! It parses a few flags (`--channels`, `--size`, `--spin`), creates the
//! slab, starts the background core, and then just idles until the user
//! hits Ctrl‑C.  The program prints a short status line so you can see that
//! the core is alive and the slab is usable.

use std::process;
use std::sync::Arc;
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
    // CLI parsing – uses `clap` (add `clap = { version = "4", features = ["derive"] }`
    // to your Cargo.toml).
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

    let channels: usize = matches
        .value_of_t("channels")
        .expect("failed to parse channels");
    let size_str = matches.value_of("size").unwrap(); // required
    let size_bytes = parse_size(size_str).unwrap_or_else(|e| {
        eprintln!("Error parsing --size: {}", e);
        process::exit(1);
    });
    let spin_policy = match matches.value_of("spin").unwrap() {
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
    let slayer = UltraSlayer::<u64>::with_channels(channels, size_bytes)
        .expect("Failed to allocate UltraSlayer slab");
    slayer.set_spin_policy(spin_policy);
    slayer.spawn_slayer_core();

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
