//! Persistent configuration + canonical on-disk paths.

use crate::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// User home directory (falls back to cwd).
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Base config dir, overridable with `LOCALCODE_HOME`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("LOCALCODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/localcode"))
}

pub fn cache_dir() -> PathBuf {
    home().join(".cache/localcode")
}

pub fn models_dir() -> PathBuf {
    cache_dir().join("models")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Sandbox enforcement level for tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxLevel {
    /// Only read/search tools; no writes, no bash.
    ReadOnly,
    /// Default: edits/writes/bash confined to the workspace, network off.
    #[default]
    WorkspaceWrite,
    /// Opt-in: unconfined.
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Human-facing model name (also sent as the `model` field; llama-server ignores it).
    pub model_alias: String,
    /// Path to the GGUF weights. `None` when attaching to an external endpoint.
    #[serde(default)]
    pub model_path: Option<PathBuf>,
    /// External OpenAI-compatible endpoint (set by `--attach`); when present we do not spawn a server.
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default = "default_ctx")]
    pub n_ctx: u32,
    /// GPU layers to offload; -1 = all (Metal on this Mac).
    #[serde(default = "default_ngl")]
    pub n_gpu_layers: i32,
    #[serde(default)]
    pub sandbox: SandboxLevel,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    #[serde(default)]
    pub llama_server_bin: Option<PathBuf>,
    #[serde(default = "default_temp")]
    pub temperature: f32,
    #[serde(default = "default_port")]
    pub port: u16,
    /// KV-cache quantization. Default q8_0; NEVER q4_0 (degrades tool-calling).
    #[serde(default = "default_kv_quant")]
    pub kv_quant: String,
}

fn default_ctx() -> u32 {
    16384
}
fn default_ngl() -> i32 {
    -1
}
fn default_max_turns() -> usize {
    14
}
fn default_temp() -> f32 {
    0.2
}
fn default_port() -> u16 {
    8757
}
fn default_kv_quant() -> String {
    "q8_0".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model_alias: "qwen2.5-coder-7b-instruct".to_string(),
            model_path: None,
            endpoint: None,
            n_ctx: default_ctx(),
            n_gpu_layers: default_ngl(),
            sandbox: SandboxLevel::default(),
            max_turns: default_max_turns(),
            llama_server_bin: None,
            temperature: default_temp(),
            port: default_port(),
            kv_quant: default_kv_quant(),
        }
    }
}

impl Config {
    /// Load config from disk; `Ok(None)` if it does not exist yet (triggers onboarding).
    pub fn load() -> Result<Option<Config>> {
        let path = config_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(Some(cfg))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).with_context(|| format!("writing config {}", path.display()))?;
        Ok(())
    }
}
