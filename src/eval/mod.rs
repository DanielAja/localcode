//! Per-model tool-call eval: does the configured model pick the right tool for a
//! given request? This is the honest "can this model drive the agent?" metric.
//!
//! Each case is a single-turn prompt with the full tool set; we check the model's
//! first tool call against the expected tool. Run with `localcode eval`.

use crate::engine::{ChatMessage, Engine};
use crate::tools::Registry;
use crate::ui::style;
use crate::{toolcall, Result};

pub struct EvalCase {
    pub prompt: &'static str,
    pub expect_tool: &'static str,
}

pub const CASES: &[EvalCase] = &[
    EvalCase { prompt: "Read the file Cargo.toml.", expect_tool: "read_file" },
    EvalCase { prompt: "List the files in the current directory.", expect_tool: "list_dir" },
    EvalCase { prompt: "Search the project for the text TODO.", expect_tool: "grep" },
    EvalCase { prompt: "Find all files matching the pattern src/**/*.rs.", expect_tool: "glob" },
    EvalCase { prompt: "Create a new file called notes.txt containing the text hello.", expect_tool: "write_file" },
    EvalCase { prompt: "Run the shell command `ls -la` to list files.", expect_tool: "bash" },
];

/// Run the eval suite against the engine and print a per-case + summary report.
pub async fn run(engine: &Engine, registry: &Registry, system: &str) -> Result<()> {
    let specs = registry.specs();
    println!("{}", style::paint(style::BOLD, "tool-call eval"));
    let mut pass = 0usize;

    for case in CASES {
        let messages = vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(case.prompt),
        ];
        let (msg, _) = engine.chat(&messages, Some(&specs), None, None).await?;
        let extracted = toolcall::extract(&msg);
        let got = extracted.calls.first().map(|c| c.function.name.clone());
        let ok = got.as_deref() == Some(case.expect_tool);
        if ok {
            pass += 1;
        }
        let (mark, color) = if ok { ("✓", style::GREEN) } else { ("✗", style::RED) };
        println!(
            "  {} expected {:<11} got {:<11}  {}",
            style::paint(color, mark),
            case.expect_tool,
            got.as_deref().unwrap_or("(none)"),
            style::paint(style::GREY, case.prompt),
        );
    }

    let total = CASES.len();
    let pct = (pass as f64 / total as f64) * 100.0;
    let color = if pct >= 80.0 { style::GREEN } else if pct >= 50.0 { style::YELLOW } else { style::RED };
    println!(
        "\n{}",
        style::paint(color, &format!("tool-call accuracy: {pass}/{total} ({pct:.0}%)"))
    );
    if pct < 80.0 {
        println!(
            "{}",
            style::paint(
                style::GREY,
                "Below 80% — this model is best used in interactive (human-in-the-loop) mode. A larger model (e.g. qwen3-coder-30b-a3b) drives the agent more reliably."
            )
        );
    }
    Ok(())
}
