//! Model registry (verified tiers), recommendation, and on-disk discovery.

pub mod download;

use std::path::{Path, PathBuf};

/// A model the tool is willing to recommend/download by default (license-gated to
/// Apache-2.0 / MIT). Unverified mid-2026 models are intentionally excluded.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub alias: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub approx_gb: f64,
    /// Minimum recommended total RAM (GB) for a usable experience.
    pub min_ram_gb: f64,
    pub license: &'static str,
    pub note: &'static str,
}

pub const VERIFIED: &[ModelEntry] = &[
    ModelEntry {
        alias: "qwen2.5-coder-3b",
        repo: "bartowski/Qwen2.5-Coder-3B-Instruct-GGUF",
        file: "Qwen2.5-Coder-3B-Instruct-Q4_K_M.gguf",
        approx_gb: 2.1,
        min_ram_gb: 8.0,
        license: "Apache-2.0",
        note: "Low-end / 8 GB tier. CPU-friendly, small context.",
    },
    ModelEntry {
        alias: "qwen2.5-coder-7b",
        repo: "bartowski/Qwen2.5-Coder-7B-Instruct-GGUF",
        file: "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        approx_gb: 4.7,
        min_ram_gb: 16.0,
        license: "Apache-2.0",
        note: "16 GB unified tier (the live-test default on Apple Silicon).",
    },
    ModelEntry {
        alias: "qwen3-coder-30b-a3b",
        repo: "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
        file: "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf",
        approx_gb: 18.6,
        min_ram_gb: 32.0,
        license: "Apache-2.0",
        note: "32 GB+ default. MoE (3.3B active) → fast; native tool-calling.",
    },
];

/// Recommend the largest verified model that fits the device's RAM tier and free disk.
pub fn recommend(total_ram_gb: f64, free_disk_gb: f64) -> Option<&'static ModelEntry> {
    VERIFIED
        .iter()
        .filter(|m| m.min_ram_gb <= total_ram_gb + 0.5 && m.approx_gb * 1.15 <= free_disk_gb)
        .max_by(|a, b| a.approx_gb.partial_cmp(&b.approx_gb).unwrap_or(std::cmp::Ordering::Equal))
}

/// Find a verified entry by alias.
pub fn by_alias(alias: &str) -> Option<&'static ModelEntry> {
    VERIFIED.iter().find(|m| m.alias == alias)
}

/// Find the first `*.gguf` in a directory (used until the registry/download lands).
pub fn find_model_in_dir(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut ggufs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "gguf").unwrap_or(false))
        .collect();
    ggufs.sort();
    ggufs.into_iter().next()
}
