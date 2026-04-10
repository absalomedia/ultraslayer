use std::env;

/// Hardware configuration for DRAM channel interleaving.
pub struct ArchConfig {
    /// Byte distance between replica 0 and replica 1 within the slab.
    pub replica_offset: usize,
}

impl ArchConfig {
    /// Detects the platform and returns the optimal configuration.
    /// It allows for environment variable overrides to enable "calibration" 
    /// without recompiling, which is critical for HFT tuning.
    pub fn for_platform() -> Self {
        let default_offset = Self::detect_default_offset();
        
        // Allow override via SLAYER_REPLICA_OFFSET for empirical tuning
        let replica_offset = env::var("SLAYER_REPLICA_OFFSET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_offset);

        Self { replica_offset }
    }

    fn detect_default_offset() -> usize {
        #[cfg(target_arch = "x86_64")]
        {
            // x86_64 (Intel/AMD): 64 MiB is generally safe for crossing 
            // channel/rank/NUMA boundaries on most server SKUs.
            64 * 1024 * 1024
        }

        #[cfg(target_arch = "aarch64")]
        {
            // ARM64 (Graviton/AWS): ARM SoC memory controllers often use 
            // different interleaving policies. 1 GiB is a safer default 
            // to ensure landing on different controllers in a mesh interconnect.
            1024 * 1024 * 1024
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            // Fallback for other architectures
            128 * 1024 * 1024
        }
    }
}

