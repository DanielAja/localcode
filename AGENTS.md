# AGENTS.md

**localcode** — a 100% local, on-device CLI AI coding agent in Rust. It manages a local llama.cpp `llama-server` and drives an approval-gated agent loop with coding + web tools. No cloud, no API keys, no telemetry.

## Build / test / run
- Build: `cargo build` (release: `cargo build --release`)
- Test: `cargo test` — network tests are `#[ignore]`d; run them with `cargo test -- --ignored`
- Lint: `cargo clippy` (keep it warning-clean)
- Run: `localcode` (interactive), `localcode --print "…"` (one-shot), `localcode doctor`, `localcode eval`

## Layout (`src/`)
- `engine/` — llama-server lifecycle, OpenAI client, SSE streaming, server reuse
- `agent/` — the agentic loop (recovery/verify nudges, loop + echo guards, compaction)
- `tools/` — read/write/edit/ls/glob/grep/bash/todo + `web` (search/fetch)
- `toolcall.rs` — forgiving tool-call extraction (native → text → repair)
- `permissions/` — sandbox policy, macOS Seatbelt, colorized diff preview
- `commands.rs` — REPL slash commands (+ easter eggs)
- `session.rs` — bundled-SQLite session persistence
- `hardware/` · `models/` · `onboarding/` — scan + budget, registry/download, wizard
- `eval/` — per-model tool-call accuracy
- `cli.rs` · `ui.rs` — entry point + line-mode UI (spinner, color gating)

## Conventions
- Edits use **fail-loud** search/replace (never silent no-op or mis-edit).
- File tools are **jailed** to the workspace; `bash` runs under **Seatbelt** (network denied) at workspace-write.
- Keep it compiling and clippy-clean; add a test when you add logic (`tests/` + in-module `#[cfg(test)]`).
- Only Apache-2.0 / MIT models in the registry. Default KV-cache quant `q8_0` (never `q4_0`).
- Small local models are **human-in-the-loop** — keep approvals on; prefer a 30B on 32 GB+ for autonomy.
