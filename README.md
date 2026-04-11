# ⚡️ UltraSlayer – DRAM Refresh‑Stall Killer  

**UltraSlayer** is a lock‑free, hardware‑aware memory slab that eliminates the “DRAM refresh stall” (tREFI) tail‑latency that destroys nanosecond‑level determinism in High‑Frequency‑Trading (HFT) and other ultra‑low‑latency workloads.  

It mirrors every hot‑path object across a configurable number of physical DRAM channels and lets a dedicated **Slayer Core** race the reads in parallel, guaranteeing that at least one channel will answer before a refresh can stall the request.

> **⚠️  WARNING** – UltraSlayer uses `unsafe`, `volatile` loads/stores and a core that spins 100 % of the time.  Use it **only for the critical hot‑path** of a latency‑sensitive application.

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
| **Huge‑Page support** | Uses `MAP_HUGETLB` when available → zero TLB misses. |
| **Spin policies** | `busy` (full spin), `hybrid` (spin → yield), `sleep` (periodic pause). |
| **Side‑car (`sidecar` feature)** | Builds a `cdylib` with a tiny C‑FFI (`ul_init`, `ul_read_u64`, …). |
| **POSIX Shared‑Memory wrapper** (`src/shm.rs`) | `ShmSlab<T>` lets multiple processes map the same slab via `/dev/shm`. |
| **Criterion benchmark harness** (`benchmark` feature) | `benches/read_latency.rs` measures nanosecond read latency for 2/4/8 channels. |
| **CLI demo binary** (`cli` feature) | `src/bin/ultraslayer.rs` parses flags, creates the slab, starts the core, and idles. |
| **Zero‑copy slice view** (`src/slice.rs`) | Exposes a raw‑pointer slice without copying. |
| **Full LTO + thin‑LTO options** | Optimised release builds for the smallest, fastest binary. |

---  

## 📋 System Requirements (Linux)

| Requirement | How to satisfy |
|--------------|-----------------|
| **Linux kernel ≥ 5.10** | `uname -r` |
| **Huge Pages** (recommended) | `sudo sysctl -w vm.nr_hugepages=2048` (~ 4 GiB) |
| **≥ 2 DRAM channels** (different DIMMs / banks) | Verify with BIOS or `dmidecode`. |
| **NUMA awareness** (optional) | Use `numactl` / `taskset` to bind process + memory to the same node. |
| **CPU governor** | Set to `performance` (`cpupower frequency-set -g performance`). |
| **Root / sudo** | Needed for `chrt`, `taskset`, huge‑page config. |

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
| **CLI demo** (`src/bin/ultraslayer.rs`) | `cargo build --release --features cli` | `target/release/ultraslayer` |
| **C‑FFI side‑car** (`libultraslayer.so`) | `cargo build --release --features sidecar` | `target/release/libultraslayer.so` |
| **Benchmark harness** (Criterion) | `cargo bench --features benchmark` | Runs `benches/read_latency.rs` and prints latency tables |
| **All three** | `cargo build --release --features "cli sidecar benchmark"` | Everything compiled together |

The release profile already uses **full LTO**, `opt-level = 3`, `panic = "abort"` and a **single codegen unit** for maximum inlining.  If you prefer a faster build with virtually the same performance you can change `lto = "thin"` in `Cargo.toml`.

---  

## ▶️ Running UltraSlayer (demo binary)

```bash
# Example: 4 channels, 2 GiB per channel, busy‑spin policy,
# pinned to core 2 with real‑time FIFO priority 99.
sudo chrt -f 99 taskset -c 2 target/release/ultraslayer \
    --channels 4 \
    --size 2GiB \
    --spin busy
```

**Flags**

| Flag | Meaning |
|------|---------|
| `--channels N` | Number of DRAM mirrors (default 2). |
| `--size <bytes>` | Total slab size **per channel** (e.g. `2GiB`, `512MiB`). |
| `--spin <policy>` | `busy`, `hybrid`, or `sleep` (default `busy`). |

The program prints a short status line and then idles, keeping the designated core at 100 % utilization.  Press **Ctrl‑C** to stop.

---  

## 📊 Benchmarking  

UltraSlayer ships with two ways to benchmark latency.

### 1️⃣ Criterion read‑latency benchmark  

```bash
cargo bench --features benchmark
```

The benchmark creates slabs with 2, 4, and 8 channels, fills them with deterministic data, then performs **1 000 000 random reads** per configuration while measuring **nanosecond‑resolution latency** (HDR histogram).  Sample output:

```
read_latency/2  time:   [62.1 ns 62.3 ns 62.5 ns]
read_latency/4  time:   [61.8 ns 62.0 ns 62.2 ns]
read_latency/8  time:   [61.6 ns 61.8 ns 62.0 ns]
```

### 2️⃣ Stand‑alone micro‑benchmark binary  

```bash
cargo build --release --features benchmark
sudo chrt -f 99 taskset -c 2 target/release/benchmark \
    --channels 4 --size 2GiB --ops 1_000_000 --spin busy
```

The binary prints a concise summary:

```
Benchmark completed
  Ops          : 1,000,000
  Avg latency  : 68.3 ns
  p99          : 85.0 ns
  p99.99       : 101.2 ns
  Throughput   : 14.6 M ops/s
```

Both approaches are useful: Criterion gives statistically robust confidence intervals; the binary provides a quick “single‑run” result you can embed in scripts or CI pipelines.

---  

## 🔌 Integration Guide  

UltraSlayer is intended to be the **hot‑storage** for the most latency‑sensitive data.  Below are three common integration patterns.

### A️ Pure Rust Engine  

```rust
use std::sync::Arc;
use ultraslayer::{UltraSlayer, SpinPolicy};

fn main() {
    // 2 GiB slab, 4 mirrored channels, busy‑spin policy
    let slayer = Arc::new(
        UltraSlayer::<u64>::with_channels(4, 2 << 30)
            .expect("failed to allocate UltraSlayer")
    );
    slayer.set_spin_policy(SpinPolicy::Busy);
    slayer.spawn_slayer_core();       // start the background core
    slayer.pin_to_core(3);            // keep it on core 3

    // Hot‑path usage (example: reading a price)
    let price = slayer.read(PRICE_IDX);
    // … use `price` …
}
```

All public methods (`read`, `write`, `slice`, `stats`, `set_spin_policy`, `pin_to_core`) are in `src/lib.rs`.

### B️ Non‑Rust Languages (C / Node / Python) – **Side‑car**  

```bash
cargo build --release --features sidecar
```

You now have `target/release/libultraslayer.so`.  The exported C API (in `src/ffi.rs`) is:

| C function | Description |
|------------|-------------|
| `ul_init(uint32_t channels, size_t size_bytes)` | Allocate a new slab of `u64`. Returns an opaque handle (`void*`). |
| `ul_start_core(void* handle)` | Starts the Slayer core for the given handle. |
| `ul_set_spin_policy(void* handle, int policy)` | `0=busy`, `1=hybrid`, `2=sleep`. |
| `ul_read_u64(void* handle, size_t idx)` | Volatile read. |
| `ul_write_u64(void* handle, size_t idx, uint64_t val)` | Volatile write. |
| `ul_destroy(void* handle)` | Release the slab. |

**Python example (using `ctypes`)**

```python
import ctypes, os

lib = ctypes.CDLL("./target/release/libultraslayer.so")
lib.ul_init.argtypes = [ctypes.c_uint, ctypes.c_size_t]
lib.ul_init.restype  = ctypes.c_void_p

sl = lib.ul_init(4, 2 << 30)               # 4 channels, 2 GiB each
lib.ul_start_core(sl)

price = lib.ul_read_u64(sl, 0)             # read index 0
lib.ul_write_u64(sl, 0, price + 1)        # update

lib.ul_destroy(sl)
```

The same pattern works for Node.js (`ffi-napi`) or native C/C++.

### C️⃣ Multiple Processes – **POSIX Shared‑Memory**  

```rust
use ultraslayer::ShmSlab;

// Process A – creates the slab
let shm = ShmSlab::<u64>::create("ultra_slab", 4, 2 << 30)?;

// Fill it once
for i in 0..shm.len() {
    shm.write(i, (i as u64).wrapping_mul(7));
}

// Optional: hand the slab to UltraSlayer for the full API
let slayer = shm.into_ultraslayer();
```

```rust
// Process B – opens the same slab
let shm = ShmSlab::<u64>::open("ultra_slab", 4, 2 << 30)?;
let price = shm.read(PRICE_IDX);
```

Both processes see the same mirrored data; the Slayer core in **any** process will keep the latency guarantee.

---  

## 📁 Project Layout  

```
ultraslayer/
├─ src/
│   ├─ lib.rs            ← public UltraSlayer API
│   ├─ slab.rs           ← low‑level mirroring & volatile ops
│   ├─ arch.rs           ← CPU‑affinity helpers
│   ├─ reader.rs         ← internal read‑path logic
│   ├─ main.rs           ← optional entry point for `cargo run`
│   ├─ shm.rs            ← POSIX shared‑memory wrapper
│   ├─ ffi.rs            ← C‑FFI side‑car (feature = "sidecar")
│   └─ slice.rs          ← zero‑copy slice view
├─ benches/
│   └─ read_latency.rs   ← Criterion read‑latency benchmark
├─ src/bin/
│   ├─ ultraslayer.rs    ← CLI demo binary (feature = "cli")
│   └─ benchmark.rs      ← micro‑benchmark binary (feature = "benchmark")
├─ Cargo.toml
└─ README.md            ← this file
```

All files compile automatically when the corresponding Cargo feature is enabled.

---  

## 📜 License  

UltraSlayer is released under the **Apache License, Version 2.0**.

---  

### TL;DR – Quick start for a typical HFT node  

```bash
# 1️⃣ Reserve huge pages (once per boot)
sudo sysctl -w vm.nr_hugepages=2048

# 2️⃣ Build with the CLI demo + side‑car
cargo build --release --features "cli sidecar"

# 3️⃣ Run the demo (core 2, 4 channels, 2 GiB per channel)
sudo chrt -f 99 taskset -c 2 target/release/ultraslayer \
    --channels 4 --size 2GiB --spin busy
```

You now have a **100 % hot core** serving a **mirrored DRAM slab** that is immune to refresh‑stall tail latency.  Use the C‑FFI, the shared‑memory wrapper, or the pure Rust API to integrate UltraSlayer into any latency‑critical system.

---  

**UltraSlayer** – the practical, Rust‑native answer to Laurie Wired’s TailSlayer concept. 🚀
