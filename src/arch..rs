/// Hardware configuration for DRAM channel interleaving.
pub struct ArchConfig {
    pub replica_offset: usize,
}

impl ArchConfig {
    /// Returns configuration based on standard x86_64 server architectures.
    /// The replica_offset is designed to force the memory controller 
    /// to place the data on a different physical DRAM channel.
    pub fn for_platform() -> Self {
        Self {
            // 256 bytes is a common offset to switch channels on Intel/AMD.
            // In a production HFT environment, this may be tuned based on 
            // the specific motherboard/CPU SKU.
            replica_offset: 256, 
        }
    }
}
