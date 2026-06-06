# localcode

A 100% local, on-device CLI AI coding agent — like Claude Code / Codex CLI, but every token runs on **your** machine. No API keys, no cloud, no telemetry. It spawns and manages a local [`llama.cpp`](https://github.com/ggml-org/llama.cpp) server, talks to it over the OpenAI-compatible API, and drives an approval-gated agent loop with the usual coding tools.

> Built and validated on an Apple M4 (16 GB) with `Qwen2.5-Coder-7B` (Q4_K_M). Design notes and the full research/critique behind it are in `~/.claude/plans/ultrathink-deep-adaptive-goblet.md`.

## What works today

- **Engine**: provisions/locates `llama-server`, spawns it with the right `-ngl`/`-c`/`--jinja` flags, health-checks it, and shuts it down cleanly. `--attach <url>` to use an already-running server/Ollama instead.
- **Agent loop**: assemble → call → run tools → feed results → repeat, with `max_turns`, loop/repeat detection, error-recovery nudges, and a mandatory-verification nudge.
- **Tools**: `read_file`, `write_file`, `edit_file` (fail-loud search/replace), `list_dir`, `glob`, `grep` (gitignore-aware), `bash` (real timeout via reader-threads + kill), `todo_write`.
- **Safety**: workspace path-jail on all file ops; three-tier sandbox policy (read-only / workspace-write / full); every edit/write/bash is approval-gated with a colorized diff/command preview; `--print` non-interactive mode defaults to read-only.
- **Onboarding** (`localcode setup`): permission-primed hardware scan (or manual entry), largest-model-that-fits recommendation, resumable Hugging Face download with a progress bar, config write. Auto-mode (non-TTY) for CI.
- **Live token streaming** in line mode — responses render as they generate.
- **`doctor`** (hardware scan + conservative budget), **`models`** (Apache-2.0/MIT registry), and **`eval`** (per-model tool-call accuracy).
- 20 passing tests covering the tool-call parser, fail-loud edits, budget math, path jail, and model recommendation.

## Honest limitations (and why)

Small local models (≤ ~7B) are **human-in-the-loop assistants, not autonomous agents**. In live testing, the 7B reliably handled read-only analysis, but in fully-autonomous (`--yes`) multi-step *editing* it sometimes botched multi-line replacements, looped on tool errors, or claimed success it hadn't verified. The harness mitigates this (fail-loud edits, occurrence-disambiguation hints, recovery nudges, loop/echo guards) but cannot fully overcome model capability. **Use interactive approval mode**, and prefer a larger model (e.g. `qwen3-coder-30b-a3b`, which has a dedicated tool-call format) on a 32 GB+ machine for autonomous work.

Not yet implemented (roadmap): ratatui inline-viewport TUI (line-mode streaming works today); session persistence + context compaction; OS-level sandbox (macOS Seatbelt / Linux Landlock); NVIDIA/Metal VRAM-aware fit; architect/editor split; auto-download of `llama-server` per platform.

## Requirements

- Rust (stable) — <https://rustup.rs>
- `llama-server` from llama.cpp — `brew install llama.cpp` (macOS) or a release from <https://github.com/ggml-org/llama.cpp/releases>
- A GGUF model in `~/.cache/localcode/models/` (until the onboarding downloader lands)

## Quick start

```bash
cargo build --release

cd /path/to/your/project
localcode setup           # scan hardware → recommend a model → download → config
localcode doctor          # hardware + prerequisites
localcode                 # interactive agent (approval-gated, streaming)
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
| `--attach <url>` | use an existing OpenAI-compatible endpoint |
| `--print "<prompt>"` | run one request non-interactively (read-only unless `--yes`) |
| `--yes` | auto-approve all tool actions (trusted/non-interactive use) |

Config lives at `~/.config/localcode/config.toml` (model, ctx, `-ngl`, sandbox level, KV-cache quant — default `q8_0`, never `q4_0`).

## Architecture

`engine` (llama-server lifecycle + OpenAI client) · `toolcall` (forgiving tool-call extraction) · `tools` · `agent` (the loop) · `permissions` (policy + diff preview) · `hardware` (scan + budget) · `models` (registry) · `cli`/`ui` (line mode). See the plan file for the full design and the research it's based on.

## License

Apache-2.0.
