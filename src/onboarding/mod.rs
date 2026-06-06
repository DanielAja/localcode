//! First-run onboarding: permission-primed hardware scan (or manual fallback) →
//! model recommendation → select → resumable download → write config.

use crate::config::{self, Config};
use crate::hardware::{self, gb};
use crate::models;
use crate::ui::style;
use crate::Result;
use anyhow::anyhow;

/// True when no config exists yet (first run → onboarding should run).
pub fn first_run() -> bool {
    !config::config_path().exists()
}

/// Run the onboarding wizard and return (and persist) the resulting config.
/// Interactive when stdin is a TTY; otherwise auto-selects (scan + recommended model).
pub async fn run_wizard() -> Result<Config> {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();

    println!("{}", style::paint(style::BOLD, "Welcome to localcode — on-device AI coding."));
    println!(
        "{}\n",
        style::paint(
            style::GREY,
            "Everything runs locally on your machine. No cloud, no API keys, no telemetry."
        )
    );

    // Permission priming (local-only) — biases toward opt-in and sets expectations.
    let scan_ok = if interactive {
        inquire::Confirm::new(
            "May I read this machine's specs (RAM, free disk, OS) to recommend a model? It stays on-device.",
        )
        .with_default(true)
        .prompt()
        .unwrap_or(true)
    } else {
        true
    };

    let (total_ram_gb, free_disk_gb) = if scan_ok {
        let hw = hardware::scan();
        println!("{}\n", hw.summary());
        (gb(hw.total_ram), gb(hw.free_disk))
    } else {
        println!("{}", style::paint(style::GREY, "No problem — tell me about your machine instead."));
        let ram = inquire::CustomType::<f64>::new("Total RAM in GB?")
            .with_default(16.0)
            .prompt()
            .unwrap_or(16.0);
        let disk = inquire::CustomType::<f64>::new("Free disk space (GB) for models?")
            .with_default(20.0)
            .prompt()
            .unwrap_or(20.0);
        (ram, disk)
    };

    // Recommend the largest model that fits the RAM tier + free disk.
    let recommended = models::recommend(total_ram_gb, free_disk_gb);
    let default_idx = recommended
        .and_then(|r| models::VERIFIED.iter().position(|m| m.alias == r.alias))
        .unwrap_or(0);

    let model = if interactive {
        let options: Vec<String> = models::VERIFIED
            .iter()
            .map(|m| {
                let tag = if Some(m.alias) == recommended.map(|r| r.alias) {
                    "  ← recommended"
                } else {
                    ""
                };
                format!(
                    "{:<22} ~{:>4.1} GB  (needs ~{:.0} GB RAM)  {}{}",
                    m.alias, m.approx_gb, m.min_ram_gb, m.license, tag
                )
            })
            .collect();
        let choice = inquire::Select::new("Choose a model to download:", options.clone())
            .with_starting_cursor(default_idx)
            .prompt()
            .map_err(|e| anyhow!("model selection cancelled: {e}"))?;
        let idx = options.iter().position(|o| o == &choice).unwrap_or(default_idx);
        &models::VERIFIED[idx]
    } else {
        let m = recommended
            .ok_or_else(|| anyhow!("no verified model fits this device (free up RAM/disk, or run interactively)"))?;
        println!(
            "{}",
            style::paint(style::CYAN, &format!("Auto-selected {} for this device.", m.alias))
        );
        m
    };

    // Disk gate (model size + ~15% headroom).
    if model.approx_gb * 1.15 > free_disk_gb {
        return Err(anyhow!(
            "not enough free disk for {}: needs ~{:.1} GB, only {:.1} GB free",
            model.alias,
            model.approx_gb * 1.15,
            free_disk_gb
        ));
    }

    // Download (resumable; no-op if already cached).
    let dest = config::models_dir().join(model.file);
    if dest.exists() {
        println!("{}", style::paint(style::GREEN, &format!("Model already present: {}", dest.display())));
    } else {
        println!("Downloading {} (~{:.1} GB)…", model.file, model.approx_gb);
        models::download::download(&models::download::hf_url(model.repo, model.file), &dest).await?;
        println!("{}", style::paint(style::GREEN, "Download complete."));
    }

    // Conservative context size for tight-RAM machines.
    let n_ctx = if total_ram_gb < 12.0 { 8192 } else { 16384 };
    let cfg = Config {
        model_alias: model.alias.to_string(),
        model_path: Some(dest),
        n_ctx,
        ..Config::default()
    };
    cfg.save()?;
    println!(
        "{}",
        style::paint(style::GREY, &format!("Saved config → {}", config::config_path().display()))
    );
    Ok(cfg)
}
