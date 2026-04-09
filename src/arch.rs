/// Hardware configuration for DRAM channel interleaving.
///
/// DRAM controllers on modern Intel/AMD platforms interleave across channels
/// at a granularity of typically 256–4096 bytes depending on the memory
/// controller configuration. To reliably land two replicas on *different*
/// physical channels the offset must be large enough to cross the interleave
/// boundary. 256 bytes is insufficient on many platforms; 64 MiB guarantees
/// separation on all known x86_64 server SKUs by crossing every plausible
/// interleave stride and NUMA node boundary.
///
/// In a production HFT environment this should be measured empirically with
/// LIKWID or Intel MLC and tuned per-SKU.
pub struct ArchConfig {
    /// Byte distance between replica 0 and replica 1 within the slab.
    /// Must satisfy: replica_offset * (num_replicas - 1) + max_index * size_of::<T>()
    /// <= slab_size.
    pub replica_offset: usize,
}

impl ArchConfig {
    pub fn for_platform() -> Self {
        Self {
            // 64 MiB offset reliably crosses channel/rank/NUMA boundaries on
            // dual-channel and quad-channel Intel/AMD server platforms.
            replica_offset: 64 * 1024 * 1024,
        }
    }
}
