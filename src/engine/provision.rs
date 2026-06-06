//! Locate (and, later, auto-download) the `llama-server` binary.
//!
//! v1 resolves an existing binary: configured path → `PATH` → common install
//! locations. Auto-downloading prebuilt `ggml-org/llama.cpp` releases per-platform
//! is a planned enhancement (M5); for now we point the user at `brew install
//! llama.cpp` (or the equivalent) when it is missing.

use crate::Result;
use anyhow::anyhow;
use std::path::{Path, PathBuf};

pub fn find_llama_server(configured: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = configured {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
    }
    if let Some(p) = which("llama-server") {
        return Ok(p);
    }
    for cand in [
        "/opt/homebrew/bin/llama-server",
        "/usr/local/bin/llama-server",
        "/usr/bin/llama-server",
    ] {
        let p = PathBuf::from(cand);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "llama-server not found.\n  Install llama.cpp:\n    macOS:   brew install llama.cpp\n    Linux:   see https://github.com/ggml-org/llama.cpp/releases\n  …or set `llama_server_bin` in {}",
        crate::config::config_path().display()
    ))
}

/// Minimal `which` (avoids pulling in a crate).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
