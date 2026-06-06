//! UI abstraction so the agent loop is identical in line-mode and (later) TUI mode.

use std::io::Write;

/// A user-facing front-end for the agent loop.
pub trait Ui {
    /// Non-streamed assistant prose (used by paths that don't stream).
    fn assistant_message(&mut self, text: &str);
    /// An informational notice (status, warnings).
    fn notice(&mut self, text: &str);
    /// A tool is about to run; `summary` is a one-line human description.
    fn tool_start(&mut self, name: &str, summary: &str);
    /// A tool finished; `output` is the (already truncated) result.
    fn tool_result(&mut self, name: &str, output: &str, is_error: bool);
    /// Ask the user to approve an action. `preview` is an optional rendered diff/command.
    fn confirm(&mut self, prompt: &str, preview: Option<&str>) -> bool;

    // --- streaming (default no-ops; line-mode overrides) ---
    /// Begin a streamed assistant message.
    fn stream_start(&mut self) {}
    /// A streamed text fragment.
    fn stream_delta(&mut self, _piece: &str) {}
    /// End a streamed assistant message.
    fn stream_end(&mut self) {}
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

    use std::sync::atomic::{AtomicBool, Ordering};
    static COLOR: AtomicBool = AtomicBool::new(true);

    /// Globally enable/disable ANSI color (off for non-TTY / NO_COLOR).
    pub fn set_enabled(on: bool) {
        COLOR.store(on, Ordering::Relaxed);
    }

    pub fn paint(color: &str, s: &str) -> String {
        if COLOR.load(Ordering::Relaxed) {
            format!("{color}{s}{RESET}")
        } else {
            s.to_string()
        }
    }
}

/// Line-mode (REPL) UI: streams plain lines to stdout, prompts with inquire.
pub struct LineUi {
    /// Non-interactive (`--print`): approvals are answered by policy, never prompted.
    pub non_interactive: bool,
    /// Whether we've printed the assistant prefix for the current streamed message.
    mid_stream: bool,
    /// A "thinking…" spinner shown while waiting for the first token.
    spinner: Option<indicatif::ProgressBar>,
}

impl LineUi {
    pub fn new(non_interactive: bool) -> Self {
        LineUi { non_interactive, mid_stream: false, spinner: None }
    }

    fn clear_spinner(&mut self) {
        if let Some(pb) = self.spinner.take() {
            pb.finish_and_clear();
        }
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

    fn stream_start(&mut self) {
        self.mid_stream = false;
        if !self.non_interactive {
            let pb = indicatif::ProgressBar::new_spinner();
            pb.set_style(
                indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]),
            );
            pb.set_message(style::paint(style::GREY, "thinking…"));
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            self.spinner = Some(pb);
        }
    }

    fn stream_delta(&mut self, piece: &str) {
        self.clear_spinner();
        if !self.mid_stream {
            print!("\n{} ", style::paint(style::GREEN, "⏺"));
            self.mid_stream = true;
        }
        print!("{piece}");
        let _ = std::io::stdout().flush();
    }

    fn stream_end(&mut self) {
        self.clear_spinner();
        if self.mid_stream {
            println!();
            self.mid_stream = false;
        }
    }
}
