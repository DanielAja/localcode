//! Conservative memory budget + workspace path-jail invariants.

use localcode::hardware::HwReport;
use localcode::tools::jail;
use std::path::Path;

fn report(total_gb: u64) -> HwReport {
    let gb = 1024u64 * 1024 * 1024;
    HwReport {
        total_ram: total_gb * gb,
        avail_ram: total_gb * gb / 2,
        cpus: 10,
        os: "test".to_string(),
        arch: "test".to_string(),
        free_disk: 20 * gb,
    }
}

#[test]
fn budget_is_conservative_and_bounded() {
    let hw = report(16);
    let b = hw.memory_budget();
    assert!(b > 0 && b < hw.total_ram, "budget must be (0, total)");
    let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
    // 16 * 0.65 - 3 = ~7.4 GB
    assert!(gb > 6.0 && gb < 9.0, "budget {gb} GB out of expected range");
}

#[test]
fn tiny_ram_budget_saturates_to_zero() {
    // 2 GB * 0.65 = 1.3 GB, minus a 3 GB OS reserve → saturating_sub → 0 (no underflow).
    assert_eq!(report(2).memory_budget(), 0);
}

#[test]
fn escaping_paths_are_rejected() {
    let ws = Path::new("/home/u/project");
    assert!(jail(ws, "../secret").is_err());
    assert!(jail(ws, "/etc/passwd").is_err());
    assert!(jail(ws, "src/../../etc/shadow").is_err());
}

#[test]
fn workspace_paths_are_allowed() {
    let ws = Path::new("/home/u/project");
    let p = jail(ws, "src/main.rs").unwrap();
    assert!(p.starts_with(ws));
    let nested = jail(ws, "a/b/../c.txt").unwrap();
    assert_eq!(nested, Path::new("/home/u/project/a/c.txt"));
}
