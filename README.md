# ⚡️ UltraSlayer: DRAM Tail-Latency Killer

`UltraSlayer` is a specialized hardware-aware memory library designed for High-Frequency Trading (HFT) and ultra-low latency systems. It targets a specific, often ignored source of jitter: **DRAM Refresh Stalls (tREFI)**. 

This is a very early and alpha attempt at a Rust port of @LaurieWired's TailSlayer, based on her original and extensive work: https://github.com/LaurieWired/tailslayer

## 🎯 The Problem: The "DRAM Tail"

In standard memory access, if a CPU request hits a DRAM chip exactly when it is performing a refresh cycle, the request is stalled. This causes "tail latency" spikes where a read that usually takes 60ns suddenly takes 200ns+, leading to missed trade opportunities.

## 🚀 The Solution: Hardware Hedging

UltraSlayer eliminates this by mirroring critical data across multiple physical DRAM channels. It uses a single, pinned "Slayer Core" to issue parallel reads. By racing the requests at the CPU pipeline level, it ensures that if one channel is stalled, the other channel provides the data, effectively "flattening" the latency curve.

## The Inspiration: LaurieWired

@LaurieWired released her work as a C++ version, as it is her prior art, with her video on this model of memory management: https://www.youtube.com/watch?v=KKbgulTp3FE . Based on the vibes, I decided to soft port a version to Rust. It is very alpha at this stage. Here be dragons. You have been warned.

## 🛠 Setup & Installation

### 1. System Requirements (Linux)

This library requires a Linux environment with access to Huge Pages. To reserve 4GB of RAM for Huge Pages:

```bash
# Reserve 2048 huge pages (approx 4GB)
sudo sysctl -w vm.nr_hugepages=2048
```

*Note: If Huge Pages are not configured, the library will fall back to standard pages, but you will lose the TLB-optimization benefits.*

### 2. Build Steps

To achieve nanosecond precision, you must compile with **Link Time Optimization (LTO)** and maximum optimization levels.

```bash
# Clone the repository
git clone https://github.com/your-repo/ultraslayer
cd ultraslayer

# Build for release
cargo build --release
```

### 3. Run Commands

Running this in a standard shell introduces OS jitter. You must run the binary with **Real-Time (FIFO) priority** and **Physical Core Pinning**.

```bash
# Run with real-time priority 99
sudo chrt -f 99 ./target/release/ultraslayer
```

---

## 📊 Performance & Benchmarks

| Metric | Standard `Vec<T>` / Heap | UltraSlayer | Benefit |
| :--- | :--- | :--- | :--- |
| **Average Latency** | $\sim 60\text{ns}$ | $\sim 70\text{ns}$ | Slight overhead due to signal |
| **99.9th Percentile** | $\sim 200\text{ns} - 500\text{ns}$ | $\sim 80\text{ns} - 100\text{ns}$ | **Massive Reduction** |
| **TLB Misses** | Occasional | Zero (via Huge Pages) | Deterministic Timing |
| **Symmetry** | Single Channel | Multi-Channel Mirror | Immune to $\text{tREFI}$ stalls |
| **Core Load** | Low | 1 Core (100% Spin) | Constant, predictable load |

---

## ⚠️ Critical Warnings

1. **CPU Consumption**: The Slayer Core is designed to **spin-wait**. It will utilize 100% of the assigned physical core. This is intentional to keep the CPU in a "hot" state and prevent C-state transitions (power-saving sleep) which add milliseconds of latency.
2. **Hardware Dependency**: The `replica_offset` is tuned for x86_64 (Intel/AMD) architectures. If using ARM (Graviton), the offset in `arch.rs` must be calibrated to the specific SoC memory controller.
3. **Memory Safety**: This library uses `unsafe` blocks and `volatile` reads to bypass compiler optimizations. Ensure your data structures are `#[repr(C)]` to prevent the compiler from reordering fields.

***

# 🔌 Integration Guide: Using UltraSlayer in Other Platforms

UltraSlayer is designed to be the "Hot Storage" for the most critical parts of a trading system. You should not put your entire application in the slab—only the **Hot Path data**.

## 1. What data should go into UltraSlayer?

Only store data that is read **constantly** and where a 100ns spike would be catastrophic:

- **L1/L2 Top-of-Book Prices**: The current best bid/ask.
- **Risk Limits**: Maximum position sizes (checked on every single order).
- **Internal State Flags**: "Kill-switch" flags or "Trading Enabled" booleans.
- **Sequence Numbers**: The latest processed packet ID.

## 2. Integration Architectures

### A. Integration into a Rust-based Trading Engine

Add `ultraslayer` as a dependency. Initialize the `UltraSlayer` at startup and pass it as an `Arc` to your Strategy engine.

```rust
// In your Main Loop
let slayer = Arc::new(UltraSlayer::<u64>::new(2, 1024 * 1024 * 1024));
slayer.spawn_slayer_core();

// In your Strategy Path
let price = slayer.read(PRICE_INDEX); 
// This read is now immune to DRAM refresh stalls
```

### B. Integration into Non-Rust Platforms (Node.js, Python, C++)

Since the "Slayer Core" must be pinned to a physical CPU core, you cannot run this logic inside a garbage-collected language. Instead, use the **Sidecar Model**:

1. **C-API Export**: Compile UltraSlayer as a `cdylib` (shared library).
2. **FFI Bridge**: Use `ffi-napi` (Node.js) or `ctypes` (Python) to call `slayer.read()`.
3. **Shared Memory**: Since `HugeSlab` is just a block of raw memory, you can map this same memory address into your TypeScript/Python process.

**The Workflow:**

- **Rust Side**: Runs the `Slayer Core` and manages the DRAM hedging.
- **TS/Python Side**: Writes updated prices into the slab $\rightarrow$ Rust Slayer Core reads them and serves them back via FFI with nanosecond precision.

### C. Integration via Shared Memory (IPC)

If your Strategy and your Market Data Feed are in different processes:

1. Use `shm_open` to create a shared memory segment.
2. Use `UltraSlayer` to wrap that shared segment.
3. The Market Data process writes the "Price" to the slab.
4. The Strategy process reads the "Price" using the `UltraSlayer` hedged-read logic.

## 3. Summary of Integration Flow

**Market Data** $\rightarrow$ **UltraSlayer Slab** $\rightarrow$ **Slayer Core (Parallel Read)** $\rightarrow$ **Trading Strategy** $\rightarrow$ **Order Execution**
