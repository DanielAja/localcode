//! Command-line entry point: arg parsing + dispatch + the line-mode REPL.

use crate::agent::{self, Agent};
use crate::config::{self, Config, SandboxLevel};
use crate::engine::{
    llama_server::{LlamaServer, ServerOpts},
    provision, Engine,
};
use crate::permissions::Policy;
use crate::ui::{style, LineUi, Ui};
use crate::{eval, hardware, models, onboarding, tools};
use crate::Result;
use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "localcode",
    version = concat!(env!("CARGO_PKG_VERSION"), "  ·  your code, your machine, your rules"),
    about = "A 100% local, on-device CLI AI coding agent"
)]
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

    /// Resume the most recent session for this workspace.
    #[arg(long, global = true)]
    resume: bool,

    /// Route prompts through the architect→editor (plan-then-edit) flow.
    #[arg(long, global = true)]
    architect: bool,

    /// Use the inline-viewport TUI instead of the default line mode (interactive only).
    #[arg(long, global = true)]
    tui: bool,

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
    /// List saved sessions.
    Sessions,
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

    // Colors on only for an interactive TTY without NO_COLOR.
    style::set_enabled(std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none());

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Doctor) => cmd_doctor(),
        Some(Commands::Models) => cmd_models(),
        Some(Commands::Setup) => onboarding::run_wizard().await.map(|_| ()),
        Some(Commands::Eval) => cmd_eval(cli).await,
        Some(Commands::Serve) => cmd_serve().await,
        Some(Commands::Sessions) => cmd_sessions(),
        Some(Commands::Run) | None => cmd_run(cli).await,
    }
}

fn cmd_sessions() -> Result<()> {
    let store = crate::session::Store::open()?;
    let list = store.list(20)?;
    if list.is_empty() {
        println!("no saved sessions yet.");
        return Ok(());
    }
    println!("{}", style::paint(style::BOLD, "recent sessions:"));
    for s in list {
        println!(
            "  #{:<4} {:>2} turns  {:<9} {}",
            s.id,
            s.turns,
            crate::session::ago(s.updated_at),
            style::paint(style::GREY, &s.workspace)
        );
    }
    println!("\n{}", style::paint(style::GREY, "resume the latest for a workspace with: localcode --resume"));
    Ok(())
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
        Err(_) => match crate::engine::server_install::find_managed() {
            Some(p) => println!("Server: auto-installed llama-server at {}", p.display()),
            None => {
                let (os, arch) = (
                    crate::engine::server_install::host_os(),
                    crate::engine::server_install::host_arch(),
                );
                let can_dl = !matches!(os, crate::engine::server_install::Os::Other)
                    && !matches!(arch, crate::engine::server_install::Arch::Other);
                if can_dl {
                    println!(
                        "Server: {} ({})",
                        style::paint(style::YELLOW, "not installed — will auto-download a prebuilt on first run"),
                        style::paint(style::GREY, "or `brew install llama.cpp`"),
                    );
                } else {
                    println!("Server: {}", style::paint(style::RED, "llama-server NOT FOUND (install llama.cpp manually)"));
                }
            }
        },
    }
    let net_enforced = crate::permissions::sandbox::network_enforced(SandboxLevel::WorkspaceWrite);
    let backend = crate::permissions::sandbox::backend_label();
    println!(
        "Sandbox: bash network {}",
        if net_enforced {
            format!("DENIED via {backend} (workspace-write)")
        } else {
            format!("not OS-enforced on this platform — approval-gated + path-jail ({backend})")
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
    // Default to the proven line mode; opt into the inline-viewport TUI with --tui
    // (interactive TTY only — never under --print or a pipe).
    let use_tui = cli.tui && !non_interactive && std::io::stdout().is_terminal();
    let mut ui: Box<dyn Ui> = if use_tui {
        match crate::tui::TuiUi::new() {
            Ok(t) => Box::new(t),
            Err(e) => {
                eprintln!("{}", style::paint(style::YELLOW, &format!("TUI unavailable ({e}); using line mode")));
                Box::new(LineUi::new(non_interactive))
            }
        }
    } else {
        Box::new(LineUi::new(non_interactive))
    };
    let architect_mode = cli.architect || config.as_ref().map(|c| c.architect).unwrap_or(false);

    // Session persistence (per workspace).
    let ws_key = workspace.display().to_string();
    let store = crate::session::Store::open().ok();
    let mut session_id: Option<i64> = None;
    if let Some(st) = &store {
        if cli.resume {
            if let Ok(Some((id, msgs))) = st.latest_for(&ws_key) {
                let n = msgs.len().saturating_sub(1);
                agent.restore(msgs);
                session_id = Some(id);
                ui.notice(&format!("resumed session #{id} ({n} messages)"));
            }
        }
        if session_id.is_none() {
            let title = workspace
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ws_key.clone());
            session_id = st.create(&ws_key, &title).ok();
        }
    }

    if let Some(prompt) = cli.print {
        if architect_mode {
            agent::architect::architect_editor(&mut agent, &prompt, ui.as_mut()).await?;
        } else {
            agent.push_user(prompt);
            agent.run_turn(ui.as_mut()).await?;
        }
        if let (Some(st), Some(id)) = (&store, session_id) {
            let _ = st.save(id, &agent.snapshot());
        }
        return Ok(());
    }

    let n_ctx = config.as_ref().map(|c| c.n_ctx as usize).unwrap_or(16384);
    print_banner(ui.as_mut(), &alias, &workspace, architect_mode, use_tui);
    let prompt = format!("{} ", style::paint(style::BLUE, "you›"));
    while let Some(line) = ui.read_line(&prompt) {
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match crate::commands::handle(input, &mut agent, ui.as_mut(), &workspace, &alias, n_ctx).await {
            Ok(crate::commands::Action::Quit) => break,
            Ok(crate::commands::Action::Handled) => {}
            Ok(crate::commands::Action::Passthrough(text)) => {
                let result = if architect_mode {
                    agent::architect::architect_editor(&mut agent, &text, ui.as_mut()).await
                } else {
                    agent.push_user(text);
                    agent.run_turn(ui.as_mut()).await
                };
                if let Err(e) = result {
                    ui.notice(&format!("error: {e:#}"));
                }
            }
            Err(e) => ui.notice(&format!("error: {e:#}")),
        }
        if let (Some(st), Some(id)) = (&store, session_id) {
            let _ = st.save(id, &agent.snapshot());
        }
    }
    ui.history_block("bye.");
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
    let bin = crate::engine::server_install::ensure_llama_server(cfg.llama_server_bin.as_deref())
        .await
        .context("locating or downloading llama-server")?;
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

fn print_banner(ui: &mut dyn Ui, alias: &str, workspace: &std::path::Path, architect: bool, tui: bool) {
    let mut modes = Vec::new();
    if architect {
        modes.push("architect");
    }
    modes.push(if tui { "tui" } else { "line" });
    let banner = format!(
        "{}\n  {} {}   {} {}   {} {}\n  {}",
        style::paint(style::CYAN, "◢◤ localcode · on-device AI coding — everything stays local"),
        style::paint(style::GREY, "model"),
        style::paint(style::BOLD, alias),
        style::paint(style::GREY, "mode"),
        style::paint(style::GREY, &modes.join("+")),
        style::paint(style::GREY, "workspace"),
        style::paint(style::GREY, &workspace.display().to_string()),
        style::paint(style::GREY, "type a request, or /help · /exit to quit"),
    );
    ui.history_block(&banner);
}
