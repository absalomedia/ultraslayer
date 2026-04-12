# ⚡️ UltraSlayer – DRAM Refresh‑Stall Killer  

**UltraSlayer** is a lock‑free, hardware‑aware memory slab that eliminates the “DRAM refresh stall” (tREFI) tail‑latency that destroys nanosecond‑level determinism in High‑Frequency‑Trading (HFT) and other ultra‑low‑latency workloads.  

It mirrors every hot‑path object across a configurable number of physical DRAM channels and lets a dedicated **Slayer Core** race the reads in parallel, guaranteeing that at least one channel will answer before a refresh can stall the request.

> **⚠️  WARNING** – UltraSlayer uses `unsafe`, `volatile` loads/stores and a core that spins 100 % of the time.  Use it **only for the critical hot‑path** of a latency‑sensitive application.

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
[![Crates.io](https://img.shields.io/crates/v/ultraslayer.svg)](https://crates.io/crates/ultraslayer)
![Crates.io Total Downloads](https://img.shields.io/crates/d/ultraslayer)
[![Follow @absalomedia](https://img.shields.io/twitter/follow/absalomedia?style=social)](https://twitter.com/absalomedia)

---  

## 🎯 The Problem – DRAM “Tail”

| Situation | Latency |
|-----------|--------|
| Normal DRAM read | **≈ 60 ns** |
| Read that hits a refresh (tREFI) | **≈ 200 ns +** (spike) |

A single 200 ns jitter can be the difference between a profitable trade and a missed opportunity.

---  

## 🚀 The Solution – Hardware Hedging  

| Step | What UltraSlayer does |
|------|-----------------------|
| **Mirroring** | Stores each hot object on *N* distinct DRAM channels (different DIMMs / banks). |
| **Slayer Core** | A dedicated thread, pinned to a physical core, issues *N* parallel reads at the pipeline level. |
| **Race‑to‑first** | The first response that arrives is returned; the other reads are discarded. |
| **Deterministic latency** | Probability that **all** channels are refreshed simultaneously is 1/N → tail is dramatically reduced. |

The core spins continuously to keep the core hot and avoid C‑state exits that would re‑introduce jitter.

---  

## ❤️ Inspiration – Laurie Wired’s TailSlayer  

UltraSlayer is a **Rust port of the original TailSlayer implementation** created by **Laurie Wired**.  

* **TailSlayer (C++ version)** – <https://github.com/LaurieWired/tailslayer>  
* **Video explanation (Laurie Wired)** – <https://www.youtube.com/watch?v=KKbgulTp3FE>  

Laurie Wired’s work introduced the concept of *hardware‑level hedging* to eliminate DRAM refresh‑stall tail latency. UltraSlayer adapts that concept to safe‑ish Rust while preserving the same deterministic guarantees.

---  

## 🛠️ New Features (v0.2)

| Feature | Description |
|:-------:|-------------|
| **Configurable channel count** | `--channels N` (2‑8 mirrors). |
| **Cross-Platform Memory** | Native `mmap` (Linux) and `VirtualAlloc` (Windows) support. |
| **Huge‑Page support** | Uses `MAP_HUGETLB` on Linux $\rightarrow$ zero TLB misses. |
| **Spin policies** | `busy` (full spin), `hybrid` (spin → yield), `sleep` (periodic pause). |
| **Side‑car (`sidecar` feature)** | Builds a `cdylib` with a tiny C‑FFI (`ul_init`, `ul_read_u64`, …). |
| **POSIX Shared‑Memory wrapper** (`src/shm.rs`) | `ShmSlab<T>` lets multiple processes map the same slab via `/dev/shm` (Linux only). |
| **Criterion benchmark harness** (`benchmark` feature) | `benches/read_latency.rs` measures nanosecond read latency for 2/4/8 channels. |
| **CLI demo binary** (`cli` feature) | `examples/ultraslayer_cli.rs` parses flags, creates the slab, starts the core, and idles. |
| **Zero‑copy slice view** (`slice` feature) | `src/slice.rs` exposes a raw‑pointer slice for bulk reads without copying. |
| **Full LTO + thin‑LTO options** | Optimised release builds for the smallest, fastest binary. |

---  

## 📋 System Requirements  

| Requirement | Linux | Windows |
|--------------|-----------------|-------------------|
| **Kernel/OS** | Kernel $\ge$ 5.10 | Windows 10 / 11 |
| **Huge Pages** | `sudo sysctl -w vm.nr_hugepages=2048` | Standard Virtual Memory (Automatic) |
| **DRAM Channels** | $\ge$ 2 physical channels | $\ge$ 2 physical channels |
| **CPU Affinity** | `taskset` / `chrt` | `SetThreadAffinityMask` (via `core_affinity`) |
| **Permissions** | Root/Sudo for Huge Pages/RT priority | Administrator for certain memory flags |

---  

## 📦 Getting Started – Build & Install  

### 1️⃣ Clone the repository  

```bash
git clone https://github.com/absalomedia/ultraslayer.git
cd ultraslayer
```

### 2️⃣ Build the core library (default)  

```bash
cargo build --release
```

### 3️⃣ Optional builds  

| Goal | Cargo command | What you get |
|------|---------------|--------------|
| **CLI demo** (`examples/ultraslayer_cli.rs`) | `cargo build --release --features cli` | `target/release/examples/ultraslayer_cli` |
| **C‑FFI side‑car** (`libultraslayer.so`) | `cargo build --release --features sidecar` | `target/release/libultraslayer.so` |
| **Zero‑copy slices** (`src/slice.rs`) | `cargo build --release --features slice` | Enables the `.slice()` API in `UltraSlayer` |
| **Benchmark harness** (Criterion) | `cargo bench --features benchmark` | Runs `benches/read_latency.rs` and prints latency tables |
| **All features** | `cargo build --release --features "cli sidecar slice benchmark"` | Everything compiled together |

The release profile already uses **full LTO**, `opt-level = 3`, `panic = "abort"` and a **single codegen unit** for maximum inlining.

---  

## ▶️ Running UltraSlayer (demo binary)

### Linux (with Real‑Time priority)

```bash
# Use the binary located in the examples directory
sudo chrt -f 99 taskset -c 2 target/release/examples/ultraslayer_cli \
    --channels 4 \
    --size 2GiB \
    --spin busy
```

### Windows

```powershell
# Use the binary located in the examples directory
.\target\release\examples\ultraslayer_cli.exe --channels 4 --size 2GiB --spin busy
```

**Alternatively, run directly via Cargo:**

```bash
cargo run --release --features cli --example ultraslayer_cli -- --channels 4 --size 2GiB --spin busy
```

**Flags**

| Flag | Meaning |
|------|---------|
| `--channels N` | Number of DRAM mirrors (default 2). |
| `--size <bytes>` | Total slab size **per channel** (e.g. `2GiB`, `512MiB`). |
| `--spin <policy>` | `busy`, `hybrid`, or `sleep` (default `busy`). |

---  

## 📊 Benchmarking  

UltraSlayer ships with two ways to benchmark latency.

### 1️⃣ Criterion read‑latency benchmark  

```bash
cargo bench --features benchmark
```

The benchmark creates slabs with 2, 4, and 8 channels, fills them with deterministic data, then performs **1 000 000 random reads** per configuration while measuring **nanosecond‑resolution latency**.

### 2️⃣ Stand‑alone micro‑benchmark binary  

**Via Cargo:**

```bash
cargo run --release --features benchmark --example benchmark -- --channels 4 --size 2GiB --ops 1_000_000 --spin busy
```

**Via binary (for Real-Time priority on Linux):**

```bash
# First, build the example
cargo build --release --features benchmark

# Then run the resulting binary from the examples folder
sudo chrt -f 99 taskset -c 2 target/release/examples/benchmark \
    --channels 4 --size 2GiB --ops 1_000_000 --spin busy
```

**Windows execution:**

```powershell
.\target\release\examples\benchmark.exe --channels 4 --size 2GiB --ops 1_000_000 --spin busy
```

---  

## 🔌 Integration Guide  

### A️ Pure Rust Engine  

```rust
use std::sync::Arc;
use ultraslayer::{UltraSlayer, SpinPolicy};

fn main() {
    // 2 GiB slab, 4 mirrored channels, busy‑spin policy
    let slayer = Arc::new(
        UltraSlayer::<u64>::new(4, 2 << 30) // Direct allocation
    );
    slayer.set_spin_policy(SpinPolicy::Busy);
    slayer.spawn_slayer_core(0);       // start core on CPU 0
    slayer.pin_to_core(0);            // pin current thread to core 0

    // Hot‑path usage (example: reading a price)
    let price = slayer.read(PRICE_IDX);
    
    // Optional: Bulk read via slice (requires --features slice)
    // let view = unsafe { slayer.slice() };
    // let first_val = view[0];
}
```

All public methods (`read`, `write`, `slice`, `stats`, `set_spin_policy`, `pin_to_core`) are in `src/lib.rs`.

### B️ Non‑Rust Languages (C / Node / Python) – **Side‑car**  

```bash
cargo build --release --features sidecar
```

The exported C API (in `src/ffi.rs`) provides `ul_init`, `ul_start_core`, `ul_read_u64`, `ul_write_u64`, and `ul_destroy`.

### C️ Multiple Processes – **POSIX Shared‑Memory** (Linux Only)

```rust
use ultraslayer::ShmSlab;

// Process A – creates the slab
let shm = ShmSlab::<u64>::create("ultra_slab", 4, 2 << 30)?;
let slayer = shm.into_ultraslayer();
```

---  

## 📁 Project Layout  

```
ultraslayer/
├─ src/
│   ├─ lib.rs            ← public UltraSlayer API
│   ├─ slab.rs           ← low‑level mirroring & volatile ops
│   ├─ arch.rs           ← CPU‑affinity helpers
│   ├─ reader.rs         ← internal read‑path logic
│   ├─ main.rs           ← optional entry point
│   ├─ shm.rs            ← POSIX shared‑memory wrapper (Linux)
│   ├─ ffi.rs            ← C‑FFI side‑car (feature = "sidecar")
│   └─ slice.rs          ← zero‑copy slice view
├─ benches/
│   └─ read_latency.rs   ← Criterion read‑latency benchmark
├─ examples/
│   ├─ ultraslayer_cli.rs    ← CLI demo binary (feature = "cli")
│   └─ benchmark.rs      ← micro‑benchmark binary (feature = "benchmark")
├─ Cargo.toml
└─ README.md            ← this file
```

---  

## 📜 License  

UltraSlayer is released under the **Apache License, Version 2.0**.

---  

### TL;DR – Quick start for a typical HFT node  

```bash
# 1️⃣ Reserve huge pages (Linux only)
sudo sysctl -w vm.nr_hugepages=2048

# 2️⃣ Build with the CLI demo + side‑car + slice view
cargo build --release --features "cli sidecar slice"

# 3️⃣ Run the demo (core 2, 4 channels, 2 GiB per channel)
sudo chrt -f 99 taskset -c 2 target/release/examples/ultraslayer_cli \
    --channels 4 --size 2GiB --spin busy
```

**UltraSlayer** – the practical, Rust‑native answer to Laurie Wired’s TailSlayer concept. 🚀
