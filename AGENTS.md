# AGENTS.md

**localcode** — a 100% local, on-device CLI AI coding agent in Rust. It manages a local llama.cpp `llama-server` and drives an approval-gated agent loop with coding + web tools. No cloud, no API keys, no telemetry.

## Build / test / run
- Build: `cargo build` (release: `cargo build --release`)
- Test: `cargo test` — network tests are `#[ignore]`d; run them with `cargo test -- --ignored`
- Lint: `cargo clippy` (keep it warning-clean)
- Run: `localcode` (interactive), `localcode --print "…"` (one-shot), `localcode --tui` (inline TUI), `localcode --architect "…"` (plan-then-edit), `localcode doctor`, `localcode eval`

## Layout (`src/`)
- `engine/` — llama-server lifecycle, OpenAI client, SSE streaming, server reuse; `server_install.rs` auto-downloads a prebuilt server per platform
- `agent/` — the agentic loop (recovery/verify nudges, loop + echo guards, compaction); `architect.rs` = opt-in plan-then-edit
- `tools/` — read/write/edit/ls/glob/grep/bash/todo + `web` (search/fetch)
- `toolcall.rs` — forgiving tool-call extraction (native → text → repair)
- `permissions/` — sandbox policy, macOS Seatbelt + Linux `landlock_linux.rs` (network deny), colorized diff preview
- `commands.rs` — REPL slash commands (+ easter eggs)
- `session.rs` — bundled-SQLite session persistence
- `tui/` — opt-in inline-viewport TUI (ratatui 0.30); shares the `Ui` trait with line mode
- `hardware/` · `models/` · `onboarding/` — scan + budget, registry/download, wizard
- `eval/` — per-model tool-call accuracy
- `cli.rs` · `ui.rs` — entry point + `Ui` trait (line mode: spinner, color gating)

## Conventions
- Edits use **fail-loud** search/replace (never silent no-op or mis-edit).
- File tools are **jailed** to the workspace; `bash` runs network-denied via **Seatbelt** (macOS) / **Landlock** (Linux 6.7+, best-effort) at workspace-write.
- Both front-ends drive the agent loop through the `ui::Ui` trait — keep new output going through trait methods (`notice`/`history_block`/`tool_*`) so it works in line **and** TUI mode (raw `println!` corrupts the inline viewport).
- Keep it compiling and clippy-clean; add a test when you add logic (`tests/` + in-module `#[cfg(test)]`). Cross-platform `cfg` code (e.g. Landlock) — compile-check against the target (`cargo check --target …`).
- Only Apache-2.0 / MIT models in the registry. Default KV-cache quant `q8_0` (never `q4_0`).
- Small local models are **human-in-the-loop** — keep approvals on; prefer a 30B on 32 GB+ for autonomy. Architect/editor is opt-in (modest gain on a 7B).
