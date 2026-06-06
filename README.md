# localcode

A 100% local, on-device CLI AI coding agent — like Claude Code / Codex CLI, but every token runs on **your** machine. No API keys, no cloud, no telemetry. It spawns and manages a local [`llama.cpp`](https://github.com/ggml-org/llama.cpp) server, talks to it over the OpenAI-compatible API, and drives an approval-gated agent loop with the usual coding tools.

> Built and validated on an Apple M4 (16 GB) with `Qwen2.5-Coder-7B` (Q4_K_M). Design notes and the full research/critique behind it are in `~/.claude/plans/ultrathink-deep-adaptive-goblet.md`.

## What works today

- **Engine**: provisions/locates `llama-server`, spawns it with the right `-ngl`/`-c`/`--jinja` flags, health-checks it, and shuts it down cleanly. `--attach <url>` to use an already-running server/Ollama instead.
- **Agent loop**: assemble → call → run tools → feed results → repeat, with `max_turns`, loop/repeat detection, error-recovery nudges, and a mandatory-verification nudge.
- **Tools**: `read_file`, `write_file`, `edit_file` (fail-loud search/replace), `list_dir`, `glob`, `grep` (gitignore-aware), `bash` (real timeout), `todo_write`, plus **`web_search` + `web_fetch`** so the agent can search the web on your behalf (keyless DuckDuckGo + HTML→text).
- **Safety**: workspace path-jail on file ops; three-tier sandbox (read-only / workspace-write / full); approval-gated edits/writes/bash with colorized diff previews; **macOS Seatbelt** denies network to `bash` by default; `--print` defaults to read-only.
- **Onboarding** (`localcode setup`): permission-primed hardware scan (or manual entry), largest-model-that-fits recommendation, resumable Hugging Face download with a progress bar, config write. Auto-mode (non-TTY) for CI.
- **Slash commands**: `/help /clear /compact /context /model /sandbox /diff /review /init /web /research` — plus a few easter eggs (`/coffee`, `/zen`, `/moo`).
- **Sessions**: per-workspace persistence (bundled SQLite); `--resume` to continue, `localcode sessions` to list.
- **Efficiency**: `localcode serve` keeps a warmed model server up and other runs auto-reuse it (no reload); live token streaming + a "thinking" spinner; color gating for clean piped output.
- **`doctor` / `models` / `eval`** — `eval` reports per-model tool-call accuracy (the 7B scores 6/6 on tool selection).
- 20 tests (parser, fail-loud edits, budget, path jail, recommendation) + opt-in network tests for web/download; clippy-clean.

## Honest limitations (and why)

Small local models (≤ ~7B) are **human-in-the-loop assistants, not autonomous agents**. In live testing, the 7B reliably handled read-only analysis, but in fully-autonomous (`--yes`) multi-step *editing* it sometimes botched multi-line replacements, looped on tool errors, or claimed success it hadn't verified. The harness mitigates this (fail-loud edits, occurrence-disambiguation hints, recovery nudges, loop/echo guards) but cannot fully overcome model capability. **Use interactive approval mode**, and prefer a larger model (e.g. `qwen3-coder-30b-a3b`, which has a dedicated tool-call format) on a 32 GB+ machine for autonomous work.

Not yet implemented (roadmap): ratatui inline-viewport TUI (the streaming line mode is the shipped UX today); Linux Landlock / Windows sandbox (macOS Seatbelt is done); NVIDIA/Metal VRAM-aware fit; architect/editor split; auto-download of `llama-server` per platform.

## Requirements

- Rust (stable) — <https://rustup.rs>
- `llama-server` from llama.cpp — `brew install llama.cpp` (macOS) or a release from <https://github.com/ggml-org/llama.cpp/releases>
- A model — `localcode setup` downloads one sized to your machine (or drop a GGUF in `~/.cache/localcode/models/`)

## Quick start

```bash
cargo build --release

cd /path/to/your/project
localcode setup           # scan hardware → recommend a model → download → config
localcode serve &         # (optional) keep the model warm so every run is instant
localcode                 # interactive agent (approval-gated, streaming)
localcode --resume        # continue your last session in this workspace
localcode eval            # how reliably does your model pick the right tool?
localcode --print "what does this module do?"   # one-shot, read-only
```
(The first run of `localcode` triggers onboarding automatically if there's no config.)

## Commands & flags

| | |
|---|---|
| `localcode` / `localcode run` | interactive agent (default) |
| `localcode setup` | first-run onboarding: scan → pick → download → config |
| `localcode doctor` | hardware scan, resolved model, prerequisites |
| `localcode models` | list verified models |
| `localcode eval` | measure the model's tool-call accuracy |
| `localcode serve` | keep a warmed model server running |
| `localcode sessions` | list saved sessions |
| `--resume` | resume the latest session for this workspace |
| `--attach <url>` | use an existing OpenAI-compatible endpoint |
| `--print "<prompt>"` | run one request non-interactively (read-only unless `--yes`) |
| `--yes` | auto-approve all tool actions (trusted/non-interactive use) |

In the REPL, type `/help` for slash commands (`/clear`, `/compact`, `/context`, `/review`, `/init`, `/web <q>`, `/research <topic>`, `/sandbox`, …). `NO_COLOR` and non-TTY output are honored.

Config lives at `~/.config/localcode/config.toml` (model, ctx, `-ngl`, sandbox level, KV-cache quant — default `q8_0`, never `q4_0`).

## Architecture

`engine` (llama-server lifecycle + OpenAI client + SSE) · `toolcall` (forgiving extraction) · `tools` (incl. `web`) · `agent` (the loop) · `permissions` (policy + Seatbelt sandbox + diff) · `hardware` · `models` (registry/download) · `session` (sqlite) · `commands` (slash) · `eval` · `onboarding` · `cli`/`ui`. See the plan file for the full design and the research it's based on.

## License

Apache-2.0.
