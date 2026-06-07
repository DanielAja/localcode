//! Auto-download a prebuilt `llama-server` from `ggml-org/llama.cpp` releases.
//!
//! Grounded in the real release-asset layout (verified against tags b9551 / b6000):
//! - Asset NAMES drift: the extension flipped `.zip` ↔ `.tar.gz` and the CUDA
//!   version moved `cu12.2.0` → `12.4` → `13.3` across builds — so we REGEX-match
//!   the distinctive substring (`bin-macos-arm64`, `bin-ubuntu-x64`,
//!   `bin-win-cuda-13`, …) and never hardcode a full filename.
//! - It is NOT one static binary: `llama-server` ships with co-located shared libs
//!   (libllama, libggml, libggml-metal/cuda, libmtmd, …) that load via an
//!   `@loader_path` / `$ORIGIN` rpath, so we extract everything into one dir and run
//!   the binary by its real absolute path (the engine also sets the lib-path env).
//! - Unix needs the exec bit; macOS needs the Gatekeeper quarantine xattr stripped.
//!
//! The GitHub API call needs a `User-Agent` (else 403) and is unauthenticated
//! (60 req/hr/IP) — we hit it only when no local `llama-server` exists.

use crate::Result;
use anyhow::{anyhow, Context};
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const RELEASES_LATEST: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
const UA: &str = "localcode (+https://github.com/DanielAja/localcode)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Macos,
    Linux,
    Windows,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    Cpu,
    Cuda,
    Vulkan,
    Metal,
}

pub fn host_os() -> Os {
    match std::env::consts::OS {
        "macos" => Os::Macos,
        "linux" => Os::Linux,
        "windows" => Os::Windows,
        _ => Os::Other,
    }
}

pub fn host_arch() -> Arch {
    match std::env::consts::ARCH {
        "x86_64" => Arch::X64,
        "aarch64" | "arm" => Arch::Arm64,
        _ => Arch::Other,
    }
}

/// Default accelerator for the host, overridable with `LOCALCODE_LLAMA_ACCEL`.
pub fn host_accel(os: Os) -> Accel {
    if let Some(v) = std::env::var_os("LOCALCODE_LLAMA_ACCEL") {
        match v.to_string_lossy().to_ascii_lowercase().as_str() {
            "cpu" => return Accel::Cpu,
            "cuda" => return Accel::Cuda,
            "vulkan" => return Accel::Vulkan,
            "metal" => return Accel::Metal,
            _ => {}
        }
    }
    match os {
        Os::Macos => Accel::Metal, // baked into the macos-arm64/x64 archive
        _ => Accel::Cpu,           // conservative default; GPU is opt-in via env
    }
}

/// Ordered, most-specific-first match patterns for the *server* asset.
/// All accelerated prebuilts are x64-only, so arm64 always resolves to the plain
/// arch asset. Falls back to CPU where a GPU build is unavailable.
fn candidate_patterns(os: Os, arch: Arch, accel: Accel) -> Vec<Regex> {
    let rx = |s: &str| Regex::new(s).expect("static regex");
    match os {
        Os::Macos => match arch {
            Arch::Arm64 => vec![rx(r"bin-macos-arm64\.")],
            _ => vec![rx(r"bin-macos-x64\."), rx(r"bin-macos-arm64\.")],
        },
        Os::Linux => {
            let base = match arch {
                Arch::Arm64 => rx(r"bin-ubuntu-arm64\."),
                _ => rx(r"bin-ubuntu-x64\."),
            };
            // Accelerated Linux prebuilts (Vulkan) are x64-only; there is no Linux
            // CUDA prebuilt, so NVIDIA maps to Vulkan, then CPU.
            if matches!(arch, Arch::X64) && matches!(accel, Accel::Vulkan | Accel::Cuda) {
                vec![rx(r"bin-ubuntu-vulkan-x64\."), base]
            } else {
                vec![base]
            }
        }
        Os::Windows => match accel {
            Accel::Cuda => vec![
                rx(r"bin-win-cuda-13[.-]"),
                rx(r"bin-win-cuda-12[.-]"),
                rx(r"bin-win-cuda-[0-9]+[.-]"),
                rx(r"bin-win-vulkan-x64\."),
                rx(r"bin-win-cpu-x64\."),
            ],
            Accel::Vulkan => vec![rx(r"bin-win-vulkan-x64\."), rx(r"bin-win-cpu-x64\.")],
            _ => vec![rx(r"bin-win-cpu-x64\.")],
        },
        Os::Other => vec![],
    }
}

/// Pick the server archive asset name from a release's asset list.
/// Skips the separate `cudart-*` runtime archive (handled separately).
pub fn pick_asset(names: &[String], os: Os, arch: Arch, accel: Accel) -> Option<&str> {
    let pats = candidate_patterns(os, arch, accel);
    for pat in &pats {
        if let Some(n) = names
            .iter()
            .find(|n| !n.starts_with("cudart-") && pat.is_match(n))
        {
            return Some(n.as_str());
        }
    }
    None
}

/// For a chosen Windows CUDA server asset, the matching CUDA runtime archive
/// (`cudart-llama-bin-win-cuda-<ver>-x64.zip`) that must be extracted alongside it.
pub fn matching_cudart<'a>(server_asset: &str, names: &'a [String]) -> Option<&'a str> {
    let ver = Regex::new(r"bin-win-cuda-([0-9.]+)-")
        .ok()?
        .captures(server_asset)?
        .get(1)?
        .as_str()
        .to_string();
    names
        .iter()
        .find(|n| n.starts_with("cudart-") && n.contains(&format!("cuda-{ver}-")))
        .map(|s| s.as_str())
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Where we keep auto-downloaded servers: `<cache>/llama/<tag>/`.
fn install_root() -> PathBuf {
    crate::config::cache_dir().join("llama")
}

/// Find a `llama-server` we previously auto-installed (newest tag wins).
pub fn find_managed() -> Option<PathBuf> {
    let root = install_root();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs.into_iter().rev() {
        if let Some(bin) = find_binary(&dir) {
            return Some(bin);
        }
    }
    None
}

/// Recursively locate the `llama-server` executable under `dir`.
fn find_binary(dir: &Path) -> Option<PathBuf> {
    let target = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(p);
            }
        }
    }
    None
}

/// Resolve a `llama-server`: an existing one (PATH / config / common locations),
/// else a previously auto-installed one, else download a prebuilt release.
pub async fn ensure_llama_server(configured: Option<&Path>) -> Result<PathBuf> {
    if let Ok(p) = super::provision::find_llama_server(configured) {
        return Ok(p);
    }
    if let Some(p) = find_managed() {
        return Ok(p);
    }
    download_prebuilt().await
}

/// Download + extract the right prebuilt server for this host.
pub async fn download_prebuilt() -> Result<PathBuf> {
    let os = host_os();
    let arch = host_arch();
    let accel = host_accel(os);
    if matches!(os, Os::Other) || matches!(arch, Arch::Other) {
        return Err(anyhow!(
            "no prebuilt llama-server for {}/{} — install it manually (see `localcode doctor`)",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }

    let release = fetch_latest_release().await?;
    let names: Vec<String> = release.assets.iter().map(|a| a.name.clone()).collect();
    let server_name = pick_asset(&names, os, arch, accel).ok_or_else(|| {
        anyhow!(
            "no matching llama-server asset in release {} for {:?}/{:?}/{:?}. Assets: {:?}",
            release.tag_name,
            os,
            arch,
            accel,
            names
        )
    })?;

    let dest_dir = install_root().join(&release.tag_name);
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    eprintln!(
        "downloading prebuilt llama-server {} ({server_name})…",
        release.tag_name
    );
    extract_asset(&release, server_name, &dest_dir).await?;

    // Windows CUDA needs the separate cudart runtime archive co-located.
    if matches!(os, Os::Windows) && matches!(accel, Accel::Cuda) {
        if let Some(cudart) = matching_cudart(server_name, &names) {
            eprintln!("downloading CUDA runtime ({cudart})…");
            extract_asset(&release, cudart, &dest_dir).await?;
        }
    }

    let bin = find_binary(&dest_dir)
        .ok_or_else(|| anyhow!("llama-server not found in extracted archive {}", dest_dir.display()))?;
    finalize_binary(&bin, &dest_dir);
    eprintln!("installed llama-server at {}", bin.display());
    Ok(bin)
}

async fn fetch_latest_release() -> Result<Release> {
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .context("building github client")?;
    let resp = client
        .get(RELEASES_LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("querying llama.cpp latest release")?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(anyhow!(
            "GitHub API rate-limited (unauthenticated 60/hr). Try again later or install llama-server manually."
        ));
    }
    if !status.is_success() {
        return Err(anyhow!("GitHub API returned {status}"));
    }
    let body = resp.text().await.context("reading release JSON")?;
    serde_json::from_str(&body).context("parsing release JSON")
}

/// Download one asset archive and extract it into `dest_dir`.
async fn extract_asset(release: &Release, asset_name: &str, dest_dir: &Path) -> Result<()> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("asset {asset_name} vanished from release"))?;
    let _ = asset.size; // (informational; size is shown by the download progress bar)
    let archive = install_root().join(asset_name);
    crate::models::download::download(&asset.browser_download_url, &archive)
        .await
        .with_context(|| format!("downloading {}", asset.browser_download_url))?;
    extract_archive(&archive, dest_dir)
        .with_context(|| format!("extracting {}", archive.display()))?;
    let _ = std::fs::remove_file(&archive); // reclaim disk; we keep the extracted dir
    Ok(())
}

/// Extract a `.tar.gz` or `.zip` archive into `dest_dir`.
fn extract_archive(archive: &Path, dest_dir: &Path) -> Result<()> {
    let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let f = std::fs::File::open(archive)?;
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let gz = flate2::read::GzDecoder::new(f);
        let mut ar = tar::Archive::new(gz);
        ar.unpack(dest_dir)?;
    } else if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(f)?;
        zip.extract(dest_dir)?;
    } else {
        return Err(anyhow!("unsupported archive type: {name}"));
    }
    Ok(())
}

/// Make the binary runnable: exec bit on unix, strip Gatekeeper quarantine on macOS.
fn finalize_binary(bin: &Path, dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(bin) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(bin, perms);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Unsigned downloaded binaries are quarantined; clear it so they run.
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(dir)
            .status();
    }
    let _ = (bin, dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real asset list from llama.cpp release b9551.
    fn b9551() -> Vec<String> {
        [
            "llama-b9551-bin-macos-arm64.tar.gz",
            "llama-b9551-bin-macos-x64.tar.gz",
            "llama-b9551-bin-ubuntu-x64.tar.gz",
            "llama-b9551-bin-ubuntu-vulkan-x64.tar.gz",
            "llama-b9551-bin-ubuntu-arm64.tar.gz",
            "llama-b9551-bin-ubuntu-rocm-7.2-x64.tar.gz",
            "llama-b9551-bin-win-cpu-x64.zip",
            "llama-b9551-bin-win-cuda-12.4-x64.zip",
            "llama-b9551-bin-win-cuda-13.3-x64.zip",
            "llama-b9551-bin-win-vulkan-x64.zip",
            "llama-b9551-bin-win-hip-radeon-x64.zip",
            "cudart-llama-bin-win-cuda-12.4-x64.zip",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn picks_macos_arm64() {
        let names = b9551();
        let a = pick_asset(&names, Os::Macos, Arch::Arm64, Accel::Metal);
        assert_eq!(a, Some("llama-b9551-bin-macos-arm64.tar.gz"));
    }

    #[test]
    fn picks_linux_cpu_not_vulkan() {
        let names = b9551();
        let a = pick_asset(&names, Os::Linux, Arch::X64, Accel::Cpu);
        assert_eq!(a, Some("llama-b9551-bin-ubuntu-x64.tar.gz"));
    }

    #[test]
    fn linux_cuda_falls_back_to_vulkan() {
        // No Linux CUDA prebuilt exists → NVIDIA maps to Vulkan.
        let names = b9551();
        let a = pick_asset(&names, Os::Linux, Arch::X64, Accel::Cuda);
        assert_eq!(a, Some("llama-b9551-bin-ubuntu-vulkan-x64.tar.gz"));
    }

    #[test]
    fn picks_newest_windows_cuda_and_skips_cudart() {
        let names = b9551();
        let a = pick_asset(&names, Os::Windows, Arch::X64, Accel::Cuda).unwrap();
        assert_eq!(a, "llama-b9551-bin-win-cuda-13.3-x64.zip");
        assert!(!a.starts_with("cudart-"));
        // CUDA runtime is matched to the chosen server's version.
        let rt = matching_cudart("llama-b9551-bin-win-cuda-12.4-x64.zip", &names);
        assert_eq!(rt, Some("cudart-llama-bin-win-cuda-12.4-x64.zip"));
    }

    #[test]
    fn windows_cpu_default() {
        let names = b9551();
        let a = pick_asset(&names, Os::Windows, Arch::X64, Accel::Cpu);
        assert_eq!(a, Some("llama-b9551-bin-win-cpu-x64.zip"));
    }

    #[test]
    fn arm64_linux_ignores_x64_accel() {
        // arm64 has no accelerated prebuilt → always the plain arm64 asset.
        let names = b9551();
        let a = pick_asset(&names, Os::Linux, Arch::Arm64, Accel::Vulkan);
        assert_eq!(a, Some("llama-b9551-bin-ubuntu-arm64.tar.gz"));
    }

    #[test]
    fn extracts_targz_and_finds_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("srv.tar.gz");
        // Build a tiny tar.gz containing `llama-bX/llama-server`.
        {
            let f = std::fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            let data = b"#!/bin/sh\necho fake llama-server\n";
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "llama-bX/llama-server", &data[..]).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest).unwrap();
        let bin = find_binary(&dest).expect("binary located after extraction");
        assert!(bin.ends_with("llama-server"));
        finalize_binary(&bin, &dest);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&bin).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "binary should be executable");
        }
    }

    #[test]
    fn extracts_zip_and_finds_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("srv.zip");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            use std::io::Write;
            zw.start_file("build/bin/llama-server", opts).unwrap();
            zw.write_all(b"fake").unwrap();
            zw.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest).unwrap();
        assert!(find_binary(&dest).is_some(), "binary located in nested zip dir");
    }
}
