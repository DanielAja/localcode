//! Command-line entry point: arg parsing + dispatch + the line-mode REPL.

use crate::agent::{self, Agent};
use crate::config::{self, Config, SandboxLevel};
use crate::engine::{
    llama_server::{LlamaServer, ServerOpts},
    provision, Engine,
};
use crate::permissions::Policy;
use crate::ui::{style, LineUi};
use crate::{eval, hardware, models, onboarding, tools};
use crate::Result;
use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "localcode", version, about = "A 100% local, on-device CLI AI coding agent")]
pub struct Cli {
    /// Talk to an already-running OpenAI-compatible endpoint instead of spawning one
    /// (e.g. http://127.0.0.1:8080 for an external llama-server or Ollama).
    #[arg(long, global = true)]
    attach: Option<String>,

    /// Auto-approve all tool actions (dangerous; for trusted/non-interactive use).
    #[arg(long, global = true)]
    yes: bool,

    /// Run a single prompt non-interactively and exit (line mode, no approvals UI).
    #[arg(long, value_name = "PROMPT", global = true)]
    print: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the interactive coding agent (default).
    Run,
    /// Show the hardware scan, resolved model, and prerequisites.
    Doctor,
    /// List the verified models the tool can use.
    Models,
    /// Run first-run setup: hardware scan → model pick → download → config.
    Setup,
    /// Measure how reliably the configured model picks the right tool.
    Eval,
    /// Keep a warmed model server running so other runs start instantly.
    Serve,
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Doctor) => cmd_doctor(),
        Some(Commands::Models) => cmd_models(),
        Some(Commands::Setup) => onboarding::run_wizard().await.map(|_| ()),
        Some(Commands::Eval) => cmd_eval(cli).await,
        Some(Commands::Serve) => cmd_serve().await,
        Some(Commands::Run) | None => cmd_run(cli).await,
    }
}

async fn cmd_eval(cli: Cli) -> Result<()> {
    let config = Config::load()?;
    let (engine, alias) = build_engine(&cli, config.as_ref()).await?;
    let workspace = std::env::current_dir()?;
    let registry = tools::default_registry();
    let system = crate::agent::DEFAULT_SYSTEM.replace("{WORKSPACE}", &workspace.display().to_string());
    println!("model: {alias}\n");
    eval::run(&engine, &registry, &system).await
}

fn cmd_doctor() -> Result<()> {
    let hw = hardware::scan();
    println!("{}\n{}", style::paint(style::BOLD, "localcode doctor"), hw.summary());

    let bin = provision::find_llama_server(None);
    match &bin {
        Ok(p) => println!("Server: llama-server at {}", p.display()),
        Err(_) => println!("Server: {}", style::paint(style::RED, "llama-server NOT FOUND (brew install llama.cpp)")),
    }
    let net_enforced = crate::permissions::sandbox::network_enforced(SandboxLevel::WorkspaceWrite);
    println!(
        "Sandbox: bash network {}",
        if net_enforced {
            "DENIED (Seatbelt, workspace-write)"
        } else {
            "not OS-enforced on this platform (approval-gated only)"
        }
    );

    match Config::load()? {
        Some(cfg) => {
            println!("Config: {}", config::config_path().display());
            println!("Model:  {} ({})", cfg.model_alias, cfg.model_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<attached>".into()));
        }
        None => {
            println!("Config: {} (not created yet — first run will onboard)", config::config_path().display());
            if let Some(m) = models::find_model_in_dir(&config::models_dir()) {
                println!("Model:  found {} in cache", m.display());
            } else {
                println!("Model:  none downloaded yet in {}", config::models_dir().display());
            }
        }
    }
    Ok(())
}

fn cmd_models() -> Result<()> {
    println!("{}", style::paint(style::BOLD, "Verified models (Apache-2.0 / MIT):"));
    for m in models::VERIFIED {
        println!(
            "  {:<22} ~{:>4.1} GB  min RAM {:>4.0} GB  {}\n      {}",
            m.alias, m.approx_gb, m.min_ram_gb, m.license, m.note
        );
    }
    Ok(())
}

async fn cmd_run(cli: Cli) -> Result<()> {
    let workspace = std::env::current_dir()?
        .canonicalize()
        .context("resolving current directory as workspace")?;
    let mut config = Config::load()?;

    let non_interactive = cli.print.is_some();
    // First run (no config), interactive, not attaching → onboard before continuing.
    if config.is_none() && cli.attach.is_none() && !non_interactive {
        config = Some(onboarding::run_wizard().await?);
    }
    let (engine, alias) = build_engine(&cli, config.as_ref()).await?;

    let registry = tools::default_registry();
    let sandbox = if non_interactive {
        // Non-interactive: default to read-only unless --yes is given.
        if cli.yes { SandboxLevel::WorkspaceWrite } else { SandboxLevel::ReadOnly }
    } else {
        config.as_ref().map(|c| c.sandbox).unwrap_or_default()
    };
    let mut policy = Policy::new(sandbox);
    if cli.yes {
        policy.yolo = true;
    }
    let max_turns = config.as_ref().map(|c| c.max_turns).unwrap_or(14);
    let bash_timeout = Duration::from_secs(120);

    let system = agent::DEFAULT_SYSTEM.replace("{WORKSPACE}", &workspace.display().to_string());
    let mut agent = Agent::new(
        engine,
        registry,
        policy,
        workspace.clone(),
        system,
        max_turns,
        bash_timeout,
        true, // autonomous: nudge the model back to tools if it narrates
    );
    let mut ui = LineUi::new(non_interactive);

    if let Some(prompt) = cli.print {
        agent.push_user(prompt);
        agent.run_turn(&mut ui).await?;
        return Ok(());
    }

    let n_ctx = config.as_ref().map(|c| c.n_ctx as usize).unwrap_or(16384);
    print_banner(&alias, &workspace);
    while let Some(line) = read_line(&format!("{} ", style::paint(style::BLUE, "you›"))) {
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match crate::commands::handle(input, &mut agent, &mut ui, &workspace, &alias, n_ctx).await {
            Ok(crate::commands::Action::Quit) => break,
            Ok(crate::commands::Action::Handled) => {}
            Ok(crate::commands::Action::Passthrough(text)) => {
                agent.push_user(text);
                if let Err(e) = agent.run_turn(&mut ui).await {
                    eprintln!("{}", style::paint(style::RED, &format!("error: {e:#}")));
                }
            }
            Err(e) => eprintln!("{}", style::paint(style::RED, &format!("error: {e:#}"))),
        }
    }
    println!("bye.");
    Ok(())
}

/// Build the engine: attach to `--attach`, reuse a running server, or spawn one.
async fn build_engine(cli: &Cli, config: Option<&Config>) -> Result<(Engine, String)> {
    let temperature = config.map(|c| c.temperature).unwrap_or(0.2);

    if let Some(url) = &cli.attach {
        let alias = config.map(|c| c.model_alias.clone()).unwrap_or_else(|| "attached".to_string());
        eprintln!("{}", style::paint(style::GREY, &format!("attaching to {url}")));
        return Ok((Engine::attached(url.clone(), alias.clone(), temperature), alias));
    }

    let default = Config::default();
    let cfg = config.unwrap_or(&default);

    // Reuse a server already listening on our port (e.g. `localcode serve`) — skips reload.
    let url = format!("http://127.0.0.1:{}", cfg.port);
    if crate::engine::ping(&url).await {
        eprintln!("{}", style::paint(style::GREY, &format!("reusing running server at {url}")));
        return Ok((Engine::attached(url, cfg.model_alias.clone(), temperature), cfg.model_alias.clone()));
    }

    let server = spawn_server(cfg).await?;
    Ok((Engine::owning(server, cfg.model_alias.clone(), temperature), cfg.model_alias.clone()))
}

/// Provision + spawn + health-check a llama-server from config.
async fn spawn_server(cfg: &Config) -> Result<LlamaServer> {
    let model_path = cfg
        .model_path
        .clone()
        .or_else(|| models::find_model_in_dir(&config::models_dir()))
        .ok_or_else(|| {
            anyhow!(
                "no model available — run `localcode setup` or put a GGUF in {}",
                config::models_dir().display()
            )
        })?;
    let bin = provision::find_llama_server(cfg.llama_server_bin.as_deref())?;
    let opts = ServerOpts {
        bin,
        model_path: model_path.clone(),
        host: "127.0.0.1".to_string(),
        port: cfg.port,
        n_ctx: cfg.n_ctx,
        n_gpu_layers: cfg.n_gpu_layers,
        kv_quant: cfg.kv_quant.clone(),
        jinja: true,
    };
    eprintln!(
        "{}",
        style::paint(
            style::GREY,
            &format!(
                "starting llama-server ({}, ctx {}, ngl {})…",
                model_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                cfg.n_ctx,
                cfg.n_gpu_layers
            )
        )
    );
    let mut server = LlamaServer::spawn(&opts).await?;
    server
        .wait_healthy(Duration::from_secs(180))
        .await
        .context("waiting for llama-server to become healthy")?;
    eprintln!("{}", style::paint(style::GREEN, "server ready."));
    Ok(server)
}

/// `localcode serve` — keep a warmed server running so other invocations are instant.
async fn cmd_serve() -> Result<()> {
    let config = Config::load()?;
    let default = Config::default();
    let cfg = config.as_ref().unwrap_or(&default);
    let url = format!("http://127.0.0.1:{}", cfg.port);
    if crate::engine::ping(&url).await {
        println!("Server already running at {url}");
        return Ok(());
    }
    let server = spawn_server(cfg).await?;
    println!(
        "{}",
        style::paint(
            style::GREEN,
            &format!("localcode server ready at {url} (model {}). Press Ctrl-C to stop.", cfg.model_alias)
        )
    );
    let _ = tokio::signal::ctrl_c().await;
    println!("\nstopping server…");
    server.shutdown().await;
    Ok(())
}

fn print_banner(alias: &str, workspace: &std::path::Path) {
    println!(
        "{}  {}\nmodel: {}   workspace: {}\nType your request, or /help. /exit to quit.\n",
        style::paint(style::BOLD, "localcode"),
        style::paint(style::GREY, "— on-device coding agent"),
        style::paint(style::CYAN, alias),
        workspace.display()
    );
}

fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(line),
        Err(_) => None,
    }
}
