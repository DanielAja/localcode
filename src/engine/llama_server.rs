//! Spawn and supervise a local `llama-server` child process.

use crate::Result;
use anyhow::{anyhow, Context};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

#[derive(Debug, Clone)]
pub struct ServerOpts {
    pub bin: PathBuf,
    pub model_path: PathBuf,
    pub host: String,
    pub port: u16,
    pub n_ctx: u32,
    pub n_gpu_layers: i32,
    /// KV-cache quant, e.g. "q8_0". Empty or "f16" → leave default (f16) and skip
    /// forcing flash-attention. NEVER "q4_0" (degrades tool calling).
    pub kv_quant: String,
    pub jinja: bool,
}

/// A supervised `llama-server`. Killed on `shutdown()` or `Drop`.
pub struct LlamaServer {
    child: Child,
    base_url: String,
    log_path: PathBuf,
}

impl LlamaServer {
    pub async fn spawn(opts: &ServerOpts) -> Result<Self> {
        let log_dir = crate::config::cache_dir();
        std::fs::create_dir_all(&log_dir).ok();
        let log_path = log_dir.join("llama-server.log");
        let log = std::fs::File::create(&log_path)
            .with_context(|| format!("creating log {}", log_path.display()))?;
        let log2 = log.try_clone()?;

        let mut cmd = Command::new(&opts.bin);
        cmd.arg("-m")
            .arg(&opts.model_path)
            .arg("--host")
            .arg(&opts.host)
            .arg("--port")
            .arg(opts.port.to_string())
            .arg("-c")
            .arg(opts.n_ctx.to_string())
            .arg("-ngl")
            .arg(opts.n_gpu_layers.to_string());
        if opts.jinja {
            cmd.arg("--jinja");
        }
        let quantized = !opts.kv_quant.is_empty() && opts.kv_quant != "f16";
        if quantized {
            cmd.arg("-ctk")
                .arg(&opts.kv_quant)
                .arg("-ctv")
                .arg(&opts.kv_quant)
                .arg("-fa")
                .arg("on");
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2))
            .kill_on_drop(true);

        let child = cmd
            .spawn()
            .with_context(|| format!("spawning {}", opts.bin.display()))?;

        let host = if opts.host == "0.0.0.0" { "127.0.0.1" } else { &opts.host };
        let base_url = format!("http://{host}:{}", opts.port);
        Ok(LlamaServer {
            child,
            base_url,
            log_path,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn log_path(&self) -> &PathBuf {
        &self.log_path
    }

    /// Poll `/health` until the server is ready, or fail (with the tail of the log
    /// if the process died early).
    pub async fn wait_healthy(&mut self, timeout: Duration) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/health", self.base_url);
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(anyhow!(
                    "llama-server exited early ({status}). Log tail:\n{}",
                    self.log_tail(40)
                ));
            }
            if let Ok(resp) = client.get(&url).timeout(Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if start.elapsed() > timeout {
                return Err(anyhow!(
                    "llama-server not healthy within {timeout:?}. Log tail:\n{}",
                    self.log_tail(40)
                ));
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    fn log_tail(&self, lines: usize) -> String {
        let text = std::fs::read_to_string(&self.log_path).unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }

    pub async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
