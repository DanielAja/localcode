//! UI abstraction so the agent loop is identical in line-mode and (later) TUI mode.

/// A user-facing front-end for the agent loop. The line-mode implementation lives
/// here; the ratatui inline-viewport implementation is added in M4.
pub trait Ui {
    /// Final assistant prose for a turn (no tool calls), or interim prose.
    fn assistant_message(&mut self, text: &str);
    /// An informational notice (status, warnings).
    fn notice(&mut self, text: &str);
    /// A tool is about to run; `summary` is a one-line human description.
    fn tool_start(&mut self, name: &str, summary: &str);
    /// A tool finished; `output` is the (already truncated) result.
    fn tool_result(&mut self, name: &str, output: &str, is_error: bool);
    /// Ask the user to approve an action. `preview` is an optional rendered diff/command.
    fn confirm(&mut self, prompt: &str, preview: Option<&str>) -> bool;
}

/// Simple ANSI helpers (kept dependency-free).
pub mod style {
    pub const RESET: &str = "\x1b[0m";
    pub const DIM: &str = "\x1b[2m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const CYAN: &str = "\x1b[36m";
    pub const GREY: &str = "\x1b[90m";

    pub fn paint(color: &str, s: &str) -> String {
        format!("{color}{s}{RESET}")
    }
}

/// Line-mode (REPL) UI: streams plain lines to stdout, prompts with inquire.
pub struct LineUi {
    /// When true (non-interactive `--print`), approvals are answered automatically
    /// by the policy, never by prompting.
    pub non_interactive: bool,
}

impl LineUi {
    pub fn new(non_interactive: bool) -> Self {
        LineUi { non_interactive }
    }
}

impl Ui for LineUi {
    fn assistant_message(&mut self, text: &str) {
        println!("\n{} {}", style::paint(style::GREEN, "⏺"), text);
    }

    fn notice(&mut self, text: &str) {
        println!("{}", style::paint(style::YELLOW, &format!("ℹ {text}")));
    }

    fn tool_start(&mut self, _name: &str, summary: &str) {
        println!("  {} {}", style::paint(style::CYAN, "›"), summary);
    }

    fn tool_result(&mut self, _name: &str, output: &str, is_error: bool) {
        let color = if is_error { style::RED } else { style::GREY };
        for (i, line) in output.lines().enumerate() {
            if i >= 20 {
                println!("    {}", style::paint(style::GREY, "… (output truncated)"));
                break;
            }
            println!("    {}", style::paint(color, line));
        }
    }

    fn confirm(&mut self, prompt: &str, preview: Option<&str>) -> bool {
        if self.non_interactive {
            // Should not happen (policy handles auto-approve), but be safe: deny.
            return false;
        }
        if let Some(p) = preview {
            println!("\n{p}");
        }
        inquire::Confirm::new(prompt)
            .with_default(true)
            .prompt()
            .unwrap_or(false)
    }
}
