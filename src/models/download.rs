//! Resumable model download from Hugging Face with a progress bar.
//!
//! Uses reqwest streaming directly (rather than hf-hub) so we control resume
//! (HTTP Range) and progress. Downloads to `<dest>.part`, then atomically renames.

use crate::Result;
use anyhow::{anyhow, Context};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Standard Hugging Face resolve URL for a repo file on `main`.
pub fn hf_url(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// Download `url` to `dest`, resuming a `<dest>.part` if present, with a progress bar.
/// No-ops if `dest` already exists.
pub async fn download(url: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = dest.with_extension("part");

    let client = reqwest::Client::builder()
        .build()
        .context("building http client")?;

    let mut downloaded: u64 = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);

    let mut req = client.get(url);
    if downloaded > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={downloaded}-"));
    }
    let resp = req.send().await.with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(anyhow!("download failed: HTTP {status}"));
    }
    // If we requested a range but the server ignored it (200, not 206), restart.
    let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if downloaded > 0 && !resuming {
        downloaded = 0;
    }
    let total = resp.content_length().map(|c| c + downloaded);

    let pb = match total {
        Some(t) => {
            let pb = ProgressBar::new(t);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.cyan} {percent:>3}% {binary_bytes}/{binary_total_bytes} ({binary_bytes_per_sec}, ETA {eta}) {wide_bar:.cyan/blue}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            pb
        }
        None => ProgressBar::new_spinner(),
    };
    pb.set_position(downloaded);

    let mut oo = tokio::fs::OpenOptions::new();
    oo.create(true);
    if resuming {
        oo.append(true);
    } else {
        oo.write(true).truncate(true);
    }
    let mut file = oo.open(&part).await.with_context(|| format!("opening {}", part.display()))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading download stream")?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }
    file.flush().await?;
    drop(file);

    tokio::fs::rename(&part, dest)
        .await
        .with_context(|| format!("finalizing {}", dest.display()))?;
    pb.finish_and_clear();
    Ok(())
}
