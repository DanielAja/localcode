# localcode

A 100% local, on-device CLI AI coding agent — like Claude Code / Codex CLI, but every token runs on **your** machine. No API keys, no cloud, no telemetry. It spawns and manages a local [`llama.cpp`](https://github.com/ggml-org/llama.cpp) server, talks to it over the OpenAI-compatible API, and drives an approval-gated agent loop with the usual coding tools.

> Built and validated on an Apple M4 (16 GB) with `Qwen2.5-Coder-7B` (Q4_K_M). Design notes and the full research/critique behind it are in `~/.claude/plans/ultrathink-deep-adaptive-goblet.md`.

## What works today

- **Engine**: provisions/locates `llama-server` — and if it's missing, **auto-downloads the right prebuilt** from `ggml-org/llama.cpp` releases for your OS/arch/accelerator (drift-proof asset matching, co-located libs, `+x`/de-quarantine) — spawns it with the right `-ngl`/`-c`/`--jinja` flags, health-checks it, shuts it down cleanly. `--attach <url>` to use an already-running server/Ollama instead.
- **Agent loop**: assemble → call → run tools → feed results → repeat, with `max_turns`, loop/repeat detection, error-recovery nudges, and a mandatory-verification nudge.
- **Tools**: `read_file`, `write_file`, `edit_file` (fail-loud search/replace), `list_dir`, `glob`, `grep` (gitignore-aware), `bash` (real timeout), `todo_write`, plus **`web_search` + `web_fetch`** so the agent can search the web on your behalf (keyless DuckDuckGo + HTML→text).
- **Architect/editor mode** (`--architect` / `/architect <task>`): an opt-in two-pass flow — a **read-only planning pass** produces a concrete change plan, then an **editor pass** applies it. Single model, one load. Our fail-loud `edit_file` *is* the byte-for-byte "every search must match" anti-drift gate. (Off by default — the gain for ≤7B coders is modest; see limitations.)
- **Safety**: workspace path-jail on file ops; three-tier sandbox (read-only / workspace-write / full); approval-gated edits/writes/bash with colorized diff previews; **network denied to `bash` by default** — macOS **Seatbelt** and Linux **Landlock** (best-effort, Linux 6.7+); `--print` defaults to read-only.
- **Onboarding** (`localcode setup`): permission-primed hardware scan (or manual entry), largest-model-that-fits recommendation, resumable Hugging Face download with a progress bar, config write. Auto-mode (non-TTY) for CI.
- **Two front-ends**: a polished **streaming line mode** (default) and an opt-in **inline-viewport TUI** (`--tui`, ratatui 0.30) that commits finalized transcript cells into native scrollback while keeping a live input/stream area at the bottom — panic-safe terminal restore, no alternate screen.
- **Slash commands**: `/help /clear /compact /context /model /sandbox /diff /review /init /web /research /architect` — plus a few easter eggs (`/coffee`, `/zen`, `/moo`).
- **Sessions**: per-workspace persistence (bundled SQLite); `--resume` to continue, `localcode sessions` to list.
- **Efficiency**: `localcode serve` keeps a warmed model server up and other runs auto-reuse it (no reload); live token streaming + a "thinking" spinner; color gating for clean piped output.
- **`doctor` / `models` / `eval`** — `eval` reports per-model tool-call accuracy (the 7B scores 6/6 on tool selection).
- 35+ tests (parser, fail-loud edits, budget, path jail, recommendation, release-asset matching, archive extraction, TUI wrap/measure logic) + opt-in network tests; clippy-clean. The Linux Landlock module is compile-verified against the Linux target.

## Honest limitations (and why)

Small local models (≤ ~7B) are **human-in-the-loop assistants, not autonomous agents**. In live testing, the 7B reliably handled read-only analysis, but in fully-autonomous (`--yes`) multi-step *editing* it sometimes botched multi-line replacements, looped on tool errors, or claimed success it hadn't verified. The harness mitigates this (fail-loud edits, occurrence-disambiguation hints, recovery nudges, loop/echo guards) but cannot fully overcome model capability. **Use interactive approval mode**, and prefer a larger model (e.g. `qwen3-coder-30b-a3b`, which has a dedicated tool-call format) on a 32 GB+ machine for autonomous work.

The **architect/editor** split is opt-in for exactly this reason: published evidence (Aider) shows only a modest (~+4pt) lift for weak coder models doing both passes — the big wins require a frontier editor or a dedicated reasoning architect — and it doubles latency. It's wired in and the guardrails keep your files safe under model failure, but it is not a magic fix for a 7B.

Not yet implemented (roadmap): Windows OS-level FS isolation (AppContainer is high-complexity and breaks many dev tools — Windows relies on the userland path-jail + approval gate for now); NVIDIA/Metal VRAM-aware fit; an architect-only reasoning model paired with a separate editor.

## Requirements

- Rust (stable) — <https://rustup.rs>
- `llama-server` from llama.cpp — **auto-downloaded on first run** if absent (or `brew install llama.cpp` / a release from <https://github.com/ggml-org/llama.cpp/releases>). Set `LOCALCODE_LLAMA_ACCEL=cpu|cuda|vulkan|metal` to override the accelerator pick.
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
| `--architect` | route prompts through the plan-then-edit (architect→editor) flow |
| `--tui` | use the inline-viewport TUI instead of the default line mode (interactive TTY only) |

In the REPL, type `/help` for slash commands (`/clear`, `/compact`, `/context`, `/review`, `/init`, `/web <q>`, `/research <topic>`, `/architect <task>`, `/sandbox`, …). `NO_COLOR` and non-TTY output are honored.

Config lives at `~/.config/localcode/config.toml` (model, ctx, `-ngl`, sandbox level, KV-cache quant — default `q8_0`, never `q4_0`).

## Architecture

`engine` (llama-server lifecycle + auto-download + OpenAI client + SSE) · `toolcall` (forgiving extraction) · `tools` (incl. `web`) · `agent` (the loop + `architect` plan-then-edit) · `permissions` (policy + Seatbelt/Landlock sandbox + diff) · `hardware` · `models` (registry/download) · `session` (sqlite) · `commands` (slash) · `tui` (inline-viewport) · `eval` · `onboarding` · `cli`/`ui`. See the plan file for the full design and the research it's based on.

## License

Apache-2.0.
