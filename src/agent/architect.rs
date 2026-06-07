//! Architect/editor (plan-then-edit) flow — an opt-in two-pass mode.
//!
//! Research grounding (Aider's "architect mode", aider.chat/2024/09/26): splitting
//! a coding turn into a reasoning pass + a constrained edit pass gives weaker models
//! a modest but real lift (~+4pt for GPT-4o-mini-class models) and is best kept
//! OPT-IN, not default. We adapt it to our tool-based agent with a SINGLE model
//! (one load — friendly to a 16GB Mac), two instructions:
//!
//!   1. ARCHITECT — a read-only pass. The model reads the relevant files and emits
//!      a concrete change PLAN. Edits are impossible (sandbox forced to read-only),
//!      so it can only reason. We turn OFF narrate-nudges for this pass because the
//!      architect's *final* output is prose (the plan), not a tool call.
//!   2. EDITOR — a write pass. The model applies the plan with edit_file/write_file.
//!      Crucially, our fail-loud `edit_file` already enforces the byte-for-byte
//!      "every SEARCH section must EXACTLY match the file" gate that the research
//!      identifies as the dominant failure mode — a hallucinated edit fails loudly
//!      and triggers a re-read instead of silently corrupting the file.

use crate::agent::Agent;
use crate::config::SandboxLevel;
use crate::ui::Ui;
use crate::Result;

const ARCHITECT_INSTRUCTION: &str = "\
ARCHITECT MODE (read-only — you CANNOT edit files in this pass).
First READ every file relevant to the request (use read_file / grep / glob / list_dir).
Then produce a precise implementation PLAN:
- For EACH file that must change: its exact path, then bullet points naming the
  specific edits (which functions/symbols to add, change, or remove, and the nearby
  anchor lines), plus the exact names of any new symbols, imports, or signatures.
- Address ONLY the request; do not widen scope. List files in the order to edit them.
- Do not write code blocks or diffs — describe the changes in words.
End with a line containing exactly: PLAN COMPLETE";

const EDITOR_INSTRUCTION: &str = "\
EDITOR MODE. Apply the PLAN below EXACTLY — do not add, drop, or reinterpret any
change that is not in the plan. Use edit_file (copy old_string character-for-
character from the file, including nearby unique lines so it matches ONE occurrence)
and write_file for brand-new files. If a planned edit cannot be anchored to the
file, re-read the file with read_file and try again rather than guessing. When the
edits are done and the user asked to run or verify, call bash and report the REAL
output. Reply with exactly DONE when finished and verified.";

/// Run one request through the architect (plan) → editor (apply) flow.
pub async fn architect_editor(agent: &mut Agent, task: &str, ui: &mut dyn Ui) -> Result<()> {
    // ---- PASS 1: architect — read-only planning ----
    let prev_sandbox = agent.sandbox_level();
    let prev_autonomous = agent.autonomous();
    ui.notice("◆ architect: reading the code and planning (read-only)…");
    agent.set_sandbox(SandboxLevel::ReadOnly);
    agent.set_autonomous(false); // the plan is prose; don't nudge it toward tools
    agent.push_user(format!("{ARCHITECT_INSTRUCTION}\n\n# REQUEST\n{task}"));
    agent.run_turn(ui).await?;
    let plan = agent.last_assistant_text().unwrap_or_default();

    // ---- restore policy for the edit pass ----
    agent.set_sandbox(prev_sandbox);
    agent.set_autonomous(prev_autonomous);

    let plan = strip_plan_marker(plan.trim());
    if plan.is_empty() {
        ui.notice("architect produced no plan — running the task directly instead.");
        agent.push_user(task.to_string());
        return agent.run_turn(ui).await;
    }

    // ---- PASS 2: editor — apply the plan with write tools ----
    ui.notice("◆ editor: applying the plan…");
    agent.push_user(format!("{EDITOR_INSTRUCTION}\n\n# PLAN (apply exactly)\n{plan}"));
    agent.run_turn(ui).await
}

/// Drop a trailing "PLAN COMPLETE" sentinel (and surrounding blank lines).
fn strip_plan_marker(plan: &str) -> String {
    let mut out = plan.trim_end();
    if let Some(idx) = out.rfind("PLAN COMPLETE") {
        // Only strip if it is at/near the end (the sentinel), not mid-plan.
        if out[idx..].trim() == "PLAN COMPLETE" {
            out = out[..idx].trim_end();
        }
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_plan_marker;

    #[test]
    fn strips_trailing_sentinel() {
        assert_eq!(strip_plan_marker("do x\ndo y\nPLAN COMPLETE"), "do x\ndo y");
        assert_eq!(strip_plan_marker("do x\nPLAN COMPLETE\n"), "do x");
    }

    #[test]
    fn keeps_body_without_sentinel() {
        assert_eq!(strip_plan_marker("plan body"), "plan body");
    }
}
