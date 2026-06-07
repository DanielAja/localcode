//! Slash commands for the interactive REPL — the most useful commands from
//! Claude Code / Codex CLI / OpenCode, adapted for a local agent (cost is in
//! tokens, not dollars), plus a few tasteful easter eggs.

use crate::agent::Agent;
use crate::config::SandboxLevel;
use crate::ui::{style, Ui};
use crate::Result;
use std::path::Path;
use std::process::Command;

pub enum Action {
    /// Command handled; keep looping.
    Handled,
    /// Leave the REPL.
    Quit,
    /// Not a command — treat as a normal prompt for the model.
    Passthrough(String),
}

pub async fn handle(
    input: &str,
    agent: &mut Agent,
    ui: &mut dyn Ui,
    workspace: &Path,
    model_alias: &str,
    n_ctx: usize,
) -> Result<Action> {
    if !input.starts_with('/') {
        return Ok(Action::Passthrough(input.to_string()));
    }
    let body = &input[1..];
    let mut it = body.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("");
    let arg = it.next().unwrap_or("").trim();

    match cmd {
        "exit" | "quit" | "q" => return Ok(Action::Quit),
        "help" | "?" => print_help(ui),
        "clear" | "new" => {
            agent.reset();
            ui.notice("context cleared — fresh conversation.");
        }
        "context" | "status" | "tokens" | "cost" => {
            let tok = agent.approx_tokens();
            let pct = if n_ctx > 0 { (tok as f64 / n_ctx as f64 * 100.0) as u32 } else { 0 };
            ui.notice(&format!(
                "model {model_alias} · {} msgs · ~{tok} tok / {n_ctx} ctx ({pct}%) · sandbox {} · (local — no $ cost)",
                agent.message_count(),
                level_name(agent.sandbox_level()),
            ));
            if pct >= 75 {
                ui.notice("context is getting full — consider /compact or /clear.");
            }
        }
        "model" => {
            ui.notice(&format!("active model: {model_alias}"));
            ui.notice("add others with `localcode setup` / `localcode models`.");
        }
        "sandbox" => sandbox_cmd(arg, agent, ui),
        "diff" => {
            let d = git(workspace, &["diff"]);
            if d.trim().is_empty() {
                ui.notice("no uncommitted changes.");
            } else {
                ui.history_block(&d);
            }
        }
        "compact" => {
            ui.notice("compacting…");
            let n = agent.compact(if arg.is_empty() { None } else { Some(arg) }).await?;
            if n == 0 {
                ui.notice("nothing to compact yet.");
            } else {
                ui.notice("summarized earlier conversation to free context.");
            }
        }
        "review" => {
            let mut d = git(workspace, &["diff", "HEAD"]);
            if d.trim().is_empty() {
                d = git(workspace, &["diff"]);
            }
            if d.trim().is_empty() {
                ui.notice("no changes to review.");
            } else {
                agent.push_user(format!(
                    "Review this git diff for correctness bugs and obvious improvements. Be concise; cite file:line.\n\n{}",
                    truncate(&d, 12000)
                ));
                agent.run_turn(ui).await?;
            }
        }
        "init" => {
            agent.push_user(
                "Scan this project — read the README, the manifest (Cargo.toml/package.json/pyproject), and skim the main source directories — then write a concise AGENTS.md at the repo root with: what the project is, how to build/test/run it, and key conventions. Keep it under ~40 lines. Use write_file.",
            );
            agent.run_turn(ui).await?;
        }
        "web" => {
            if arg.is_empty() {
                ui.notice("usage: /web <query>");
            } else {
                match agent.run_tool("web_search", serde_json::json!({"query": arg, "max_results": 6})) {
                    Ok(o) => ui.history_block(&o.content),
                    Err(e) => ui.notice(&format!("web search failed: {e}")),
                }
            }
        }
        "research" => {
            if arg.is_empty() {
                ui.notice("usage: /research <topic>");
            } else {
                agent.push_user(format!(
                    "Research this topic for me: use web_search to find sources, web_fetch to read the most relevant 1-2 pages, then give a concise summary with the source URLs.\n\nTopic: {arg}"
                ));
                agent.run_turn(ui).await?;
            }
        }
        "architect" | "plan" => {
            if arg.is_empty() {
                ui.notice("usage: /architect <task>  — plan read-only, then apply the plan");
            } else {
                crate::agent::architect::architect_editor(agent, arg, ui).await?;
            }
        }

        // --- easter eggs: rare, instant, removable ---
        "coffee" => ui.notice("☕  brewing… done. caffeinated and unstoppable."),
        "zen" => ui.notice(zen()),
        "moo" => ui.history_block(MOO),
        "konami" => ui.notice("↑ ↑ ↓ ↓ ← → ← → B A  —  +30 lives. now go ship something."),
        "sl" => ui.notice("🚂  choo-choo! (that was /sl, not /ls — take a breath)"),

        other => ui.notice(&format!("unknown command /{other} — try /help")),
    }
    Ok(Action::Handled)
}

fn level_name(l: SandboxLevel) -> &'static str {
    match l {
        SandboxLevel::ReadOnly => "read-only",
        SandboxLevel::WorkspaceWrite => "workspace-write",
        SandboxLevel::Full => "full",
    }
}

fn sandbox_cmd(arg: &str, agent: &mut Agent, ui: &mut dyn Ui) {
    let lvl = match arg {
        "" => {
            ui.notice(&format!("sandbox: {} (set with /sandbox read-only|workspace-write|full)", level_name(agent.sandbox_level())));
            return;
        }
        "read-only" | "ro" => SandboxLevel::ReadOnly,
        "workspace-write" | "write" | "ws" => SandboxLevel::WorkspaceWrite,
        "full" | "danger" => SandboxLevel::Full,
        other => {
            ui.notice(&format!("unknown level '{other}' — use read-only|workspace-write|full"));
            return;
        }
    };
    agent.set_sandbox(lvl);
    ui.notice(&format!("sandbox set to {}.", level_name(lvl)));
}

fn print_help(ui: &mut dyn Ui) {
    let lines = [
        "/help             show this help",
        "/clear  (/new)    reset the conversation",
        "/compact [focus]  summarize history to free context",
        "/context          context / token usage",
        "/model            show the active model",
        "/sandbox [level]  show/set sandbox (read-only|workspace-write|full)",
        "/diff             show uncommitted git changes",
        "/review           review the working-tree diff",
        "/init             write an AGENTS.md for this project",
        "/web <query>      quick web search",
        "/research <topic> deep web research with sources",
        "/architect <task> plan read-only, then apply (plan-then-edit)",
        "/exit   (/quit)   leave",
    ];
    let mut block = String::new();
    for l in lines {
        block.push_str(&format!("  {}\n", style::paint(style::CYAN, l)));
    }
    block.push_str(&format!("  {}", style::paint(style::GREY, "(psst — try /coffee, /zen, /moo)")));
    ui.history_block(&block);
}

fn git(workspace: &Path, args: &[&str]) -> String {
    match Command::new("git").args(args).current_dir(workspace).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            if !o.status.success() {
                s.push_str(&String::from_utf8_lossy(&o.stderr));
            }
            s
        }
        Err(e) => format!("git not available: {e}"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}\n…[truncated]", &s[..i])
}

fn zen() -> &'static str {
    const K: &[&str] = &[
        "Simplicity is the soul of efficiency. — Austin Freeman",
        "Make it work, make it right, make it fast. — Kent Beck",
        "Weeks of coding can save you hours of planning.",
        "The best code is no code at all.",
        "Delete more than you add today.",
        "A local model is a calm model. No rate limits, no eavesdroppers.",
    ];
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    K[n % K.len()]
}

const MOO: &str = r#"
         (__)
         (oo)
   /------\/
  / |    ||
 *  /\---/\
    ~~   ~~
  ...have you mooed today?"#;
