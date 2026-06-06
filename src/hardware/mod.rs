//! Device capability scan. v1 covers RAM/disk/CPU/OS via `sysinfo` (+ std). GPU/VRAM
//! detection (objc2-metal unified memory, nvml NVIDIA) is layered in M3; for now we
//! treat Apple-Silicon unified memory as the budget via total RAM.

use sysinfo::{Disks, System};

#[derive(Debug, Clone)]
pub struct HwReport {
    pub total_ram: u64,
    pub avail_ram: u64,
    pub cpus: usize,
    pub os: String,
    pub arch: String,
    /// Free space on the volume holding the model cache.
    pub free_disk: u64,
}

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn gb(bytes: u64) -> f64 {
    bytes as f64 / GB
}

pub fn scan() -> HwReport {
    let mut sys = System::new();
    sys.refresh_memory();

    let total_ram = sys.total_memory();
    // Some platforms report available_memory as 0; fall back to total - used.
    let avail_ram = match sys.available_memory() {
        0 => total_ram.saturating_sub(sys.used_memory()),
        v => v,
    };
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let os = format!("{} {}", std::env::consts::OS, System::os_version().unwrap_or_default());
    let arch = std::env::consts::ARCH.to_string();

    let models_dir = crate::config::models_dir();
    let free_disk = free_disk_for(&models_dir);

    HwReport {
        total_ram,
        avail_ram,
        cpus,
        os,
        arch,
        free_disk,
    }
}

/// Free bytes on the volume that contains `path` (best-effort: longest matching mount).
fn free_disk_for(path: &std::path::Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    for d in &disks {
        let mp = d.mount_point();
        if path.starts_with(mp) {
            let len = mp.as_os_str().len();
            if best.map(|(l, _)| len > l).unwrap_or(true) {
                best = Some((len, d.available_space()));
            }
        }
    }
    best.map(|(_, b)| b).unwrap_or(0)
}

impl HwReport {
    /// Conservative usable memory budget for model weights + KV cache (bytes).
    /// Biased toward under-provisioning (see plan): 65% of total RAM minus an OS reserve.
    pub fn memory_budget(&self) -> u64 {
        let os_reserve = 3 * 1024 * 1024 * 1024; // ~3 GB for OS + apps
        let raw = (self.total_ram as f64 * 0.65) as u64;
        raw.saturating_sub(os_reserve)
    }

    pub fn summary(&self) -> String {
        format!(
            "OS:     {} ({})\nCPU:    {} logical cores\nRAM:    {:.1} GB total, {:.1} GB available\nBudget: ~{:.1} GB usable for a model (conservative)\nDisk:   {:.1} GB free for models",
            self.os,
            self.arch,
            self.cpus,
            gb(self.total_ram),
            gb(self.avail_ram),
            gb(self.memory_budget()),
            gb(self.free_disk),
        )
    }
}
