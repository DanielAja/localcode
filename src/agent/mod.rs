//! The agentic loop: assemble messages + tool specs, call the engine, dispatch
//! tool calls (approval-gated), feed results back, repeat until the model stops
//! calling tools or we hit a guard (max turns / loop detection).

use crate::config::SandboxLevel;
use crate::engine::{ChatMessage, Engine, Role, ToolSpec};
use crate::permissions::{Decision, Policy};
use crate::toolcall;
use crate::tools::{Registry, ToolContext, ToolOutput};
use crate::ui::Ui;
use crate::Result;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_SYSTEM: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/prompts/system.md"));

/// Cap a tool result before it goes back into context.
const MAX_TOOL_RESULT_BYTES: usize = 16_000;

pub struct Agent {
    engine: Engine,
    registry: Registry,
    specs: Vec<ToolSpec>,
    policy: Policy,
    ctx: ToolContext,
    messages: Vec<ChatMessage>,
    max_turns: usize,
    /// When true, nudge the model back to tool use if it narrates instead of acting.
    autonomous: bool,
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        engine: Engine,
        registry: Registry,
        policy: Policy,
        workspace: PathBuf,
        system_prompt: String,
        max_turns: usize,
        bash_timeout: Duration,
        autonomous: bool,
    ) -> Self {
        let specs = registry.specs();
        let messages = vec![ChatMessage::system(system_prompt)];
        let sandbox = policy.level;
        Agent {
            engine,
            registry,
            specs,
            policy,
            ctx: ToolContext { workspace, bash_timeout, sandbox },
            messages,
            max_turns,
            autonomous,
        }
    }

    pub fn policy_mut(&mut self) -> &mut Policy {
        &mut self.policy
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.messages.push(ChatMessage::user(text));
    }

    fn last_user_text(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .and_then(|m| m.content.clone())
    }

    /// Reset the conversation, keeping only the system prompt.
    pub fn reset(&mut self) {
        self.messages.truncate(1);
    }

    /// Snapshot the full message history (for session persistence).
    pub fn snapshot(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }

    /// Replace the message history (resume a saved session).
    pub fn restore(&mut self, msgs: Vec<ChatMessage>) {
        if !msgs.is_empty() {
            self.messages = msgs;
        }
    }

    /// Number of non-system messages currently in context.
    pub fn message_count(&self) -> usize {
        self.messages.len().saturating_sub(1)
    }

    /// Rough token estimate of the current context (~4 chars/token).
    pub fn approx_tokens(&self) -> usize {
        let chars: usize = self.messages.iter().filter_map(|m| m.content.as_ref()).map(|c| c.len()).sum();
        chars / 4
    }

    /// Run a tool directly (used by slash commands like /web).
    pub fn run_tool(&self, name: &str, args: serde_json::Value) -> Result<crate::tools::ToolOutput> {
        let tool = self
            .registry
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("no such tool: {name}"))?;
        tool.run(&args, &self.ctx)
    }

    pub fn sandbox_level(&self) -> SandboxLevel {
        self.policy.level
    }

    /// Change the sandbox level at runtime (keeps policy + tool context in sync).
    pub fn set_sandbox(&mut self, level: SandboxLevel) {
        self.policy.level = level;
        self.ctx.sandbox = level;
    }

    /// Compact the conversation into a summary to free context. Returns summary length.
    pub async fn compact(&mut self, focus: Option<&str>) -> Result<usize> {
        if self.messages.len() <= 2 {
            return Ok(0);
        }
        let mut convo = String::new();
        for m in self.messages.iter().skip(1) {
            let role = match m.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
                Role::System => "System",
            };
            if let Some(c) = &m.content {
                if !c.trim().is_empty() {
                    convo.push_str(&format!("{role}: {c}\n"));
                }
            }
        }
        let focus_line = focus.map(|f| format!(" Pay special attention to: {f}.")).unwrap_or_default();
        let prompt = vec![
            ChatMessage::system("You compress a coding session into a concise hand-off note so work can continue."),
            ChatMessage::user(format!(
                "Summarize the conversation below: keep key decisions, file paths, what was done, and the current state / next step.{focus_line}\n\n{convo}"
            )),
        ];
        let (msg, _) = self.engine.chat(&prompt, None, None, Some(700)).await?;
        let summary = msg.content.unwrap_or_default();
        let system = self.messages[0].clone();
        self.messages = vec![
            system,
            ChatMessage::user(format!("[Summary of earlier conversation]\n{}", summary.trim())),
        ];
        Ok(summary.len())
    }

    /// Run one user request to completion (model stops calling tools, or a guard fires).
    pub async fn run_turn(&mut self, ui: &mut dyn Ui) -> Result<()> {
        let mut recent: VecDeque<String> = VecDeque::new();
        let mut nudges: u32 = if self.autonomous { 3 } else { 0 };
        // Whether the most recent tool batch ended in an error (drives error-recovery nudges).
        let mut pending_error = false;
        // If the user asked to run/verify/test, require an actual bash call before finishing.
        let needs_verify = self.last_user_text().map(|t| mentions_verification(&t)).unwrap_or(false);
        let mut bash_ran = false;
        // NOTE: we tried forcing tool_choice="required" on recovery turns, but it makes
        // llama-server hang for this Qwen2.5-Coder-7B GGUF (a GBNF grammar pathology).
        // So recovery relies on nudges + the echo guard instead.
        for _turn in 0..self.max_turns {
            ui.stream_start();
            let (assistant, _usage) = self
                .engine
                .chat_stream(&self.messages, Some(&self.specs), None, None, |p| ui.stream_delta(p))
                .await?;
            ui.stream_end();

            let extracted = toolcall::extract(&assistant);

            // Record a normalized assistant message in history.
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: extracted.text.clone(),
                tool_calls: if extracted.calls.is_empty() {
                    None
                } else {
                    Some(extracted.calls.clone())
                },
                tool_call_id: None,
                name: None,
            });

            // No tool calls → either the final answer, or (autonomous mode) the model
            // narrated instead of acting — nudge it back to tool use (bounded budget).
            if extracted.calls.is_empty() {
                let trimmed = extracted.text.unwrap_or_default().trim().to_string();
                // A confused small model sometimes parrots the tool result back (wrapped
                // in <tool_response> tags) instead of acting; treat that as a recovery case.
                let echoing = trimmed.contains("<tool_response>") || trimmed.contains("</tool_response>");
                let want_verify = needs_verify && !bash_ran;
                if nudges > 0 && (pending_error || want_verify || echoing || looks_unfinished(&trimmed)) {
                    nudges -= 1;
                    // Error recovery takes priority over verification: if the last tool
                    // failed (or the model echoed), it must retry with a real tool call.
                    let nudge = if pending_error || echoing {
                        "Your last tool call FAILED — see the error message above. The change has NOT been applied yet. Do NOT echo or repeat the error text. Read it, correct your arguments (for an ambiguous edit, include the preceding `def` line in old_string so it matches one occurrence), and call the tool again now."
                    } else if want_verify {
                        "You have not actually run anything yet. Before finishing you MUST call the `bash` tool to run/verify the result and show its real output. Do it now, and report the ACTUAL output."
                    } else {
                        "Do not describe actions or show file/command text. Take the next step NOW by calling the appropriate tool (edit_file, write_file, bash, …). If the task is fully complete AND you verified it by running it, reply with exactly: DONE"
                    };
                    self.messages.push(ChatMessage::user(nudge));
                    continue;
                }
                if echoing {
                    ui.notice("Model kept repeating tool output and could not progress — stopping. Use interactive approval mode or a larger model (e.g. qwen3-coder-30b).");
                } else if trimmed.is_empty() {
                    ui.notice("(no response)");
                }
                return Ok(());
            }

            // (any interim prose alongside tool calls was already streamed live above)

            // Execute every tool call. We MUST answer each tool_call_id with a tool
            // message or the next request is malformed — so guards never skip that.
            let mut loop_detected = false;
            for call in &extracted.calls {
                pending_error = true; // cleared on a successful tool run below
                let name = call.function.name.clone();
                let raw_args = &call.function.arguments;

                let sig = format!("{name}:{raw_args}");
                recent.push_back(sig.clone());
                if recent.len() > 6 {
                    recent.pop_front();
                }
                if recent.iter().filter(|s| **s == sig).count() >= 3 {
                    loop_detected = true;
                }

                let tool = match self.registry.get(&name) {
                    Some(t) => t,
                    None => {
                        let msg = format!(
                            "Error: unknown tool '{name}'. Available tools: {:?}",
                            self.registry.names()
                        );
                        ui.tool_result(&name, &msg, true);
                        self.messages.push(ChatMessage::tool_result(&call.id, &name, msg));
                        continue;
                    }
                };

                let args: serde_json::Value = if raw_args.trim().is_empty() {
                    json!({})
                } else {
                    match serde_json::from_str(raw_args) {
                        Ok(v) => v,
                        Err(e) => {
                            let msg = format!(
                                "Error: arguments were not valid JSON ({e}). You sent: {raw_args}"
                            );
                            ui.tool_result(&name, &msg, true);
                            self.messages.push(ChatMessage::tool_result(&call.id, &name, msg));
                            continue;
                        }
                    }
                };

                ui.tool_start(&name, &tool.summary(&args));

                match self.policy.decide(tool) {
                    Decision::Deny => {
                        let msg = "Denied by sandbox policy (read-only mode).".to_string();
                        ui.tool_result(&name, &msg, true);
                        self.messages
                            .push(ChatMessage::tool_result(&call.id, &name, format!("Error: {msg}")));
                        continue;
                    }
                    Decision::Ask => {
                        let preview = tool.preview(&args, &self.ctx);
                        let ok = ui.confirm(&format!("Allow {}?", tool.summary(&args)), preview.as_deref());
                        if !ok {
                            let msg = "User declined this action.".to_string();
                            ui.tool_result(&name, &msg, true);
                            self.messages.push(ChatMessage::tool_result(
                                &call.id,
                                &name,
                                format!("Note: {msg} Do not retry it; choose another approach or ask the user."),
                            ));
                            continue;
                        }
                    }
                    Decision::Allow => {}
                }

                let output = match tool.run(&args, &self.ctx) {
                    Ok(o) => o,
                    Err(e) => ToolOutput::err(format!("Error: {e}")),
                };
                pending_error = output.is_error;
                if name == "bash" {
                    bash_ran = true;
                }
                ui.tool_result(&name, &truncate(&output.content, 4000), output.is_error);

                let mut content = output.content;
                if content.len() > MAX_TOOL_RESULT_BYTES {
                    let cut = floor_char_boundary(&content, MAX_TOOL_RESULT_BYTES);
                    content = format!("{}\n[result truncated]", &content[..cut]);
                }
                self.messages.push(ChatMessage::tool_result(&call.id, &name, content));
            }

            if loop_detected {
                ui.notice("Detected repeated identical tool calls — stopping to avoid a loop.");
                self.messages.push(ChatMessage::user(
                    "You are repeating the same tool call without progress. Stop, summarize what you have done and what is blocking you, and ask me how to proceed.",
                ));
                // One more model turn to let it summarize, then we return.
                ui.stream_start();
                let _ = self
                    .engine
                    .chat_stream(&self.messages, Some(&self.specs), None, None, |p| ui.stream_delta(p))
                    .await?;
                ui.stream_end();
                return Ok(());
            }
        }

        ui.notice(&format!("Reached the {}-turn limit; stopping.", self.max_turns));
        Ok(())
    }
}

/// Did the user's request ask us to run/verify/test something?
fn mentions_verification(s: &str) -> bool {
    let l = s.to_lowercase();
    ["run ", "verify", "test", "execute", "check that", "make sure", "confirm"]
        .iter()
        .any(|k| l.contains(k))
}

/// Heuristic: did the model narrate an action (and stop) instead of doing it?
fn looks_unfinished(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    if lower.trim_end_matches(['.', '!', ' ']).ends_with("done") {
        return false;
    }
    if text.contains("```") {
        return true;
    }
    const CUES: &[&str] = &[
        "let's", "let me", "i'll", "i will", "next,", "now,", "now let", "we need to",
        "then run", "run the", "please run", "you can run", "need to", "i need",
        "to read", "to check", "to find", "to determine", "to do this", "going to",
        "i should", "to fix", "to update", "to analyze", "to inspect", "first i",
    ];
    CUES.iter().any(|c| lower.contains(c))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = floor_char_boundary(s, max);
    format!("{}…", &s[..cut])
}

/// Largest char boundary <= idx (std's is unstable).
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
