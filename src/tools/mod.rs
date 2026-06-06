//! Tool implementations exposed to the model, plus the registry/dispatch.
//!
//! Edits use fail-loud search/replace (never silent no-op). File tools are jailed
//! to the workspace. Bash runs with a real timeout (reader threads + kill).

use crate::engine::ToolSpec;
use crate::Result;
use anyhow::{anyhow, bail};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Result of running a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        ToolOutput { content: content.into(), is_error: false }
    }
    pub fn err(content: impl Into<String>) -> Self {
        ToolOutput { content: content.into(), is_error: true }
    }
}

/// Execution context shared by all tools.
pub struct ToolContext {
    /// Canonical, absolute workspace root. All file ops are jailed under it.
    pub workspace: PathBuf,
    pub bash_timeout: Duration,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    /// True if the tool mutates state (edit/write/bash) → approval-gated under workspace-write.
    fn mutating(&self) -> bool;
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput>;
    /// One-line human summary for the UI / approval prompt.
    fn summary(&self, args: &Value) -> String;
    /// Optional rendered preview (e.g. a diff) shown at approval time.
    fn preview(&self, _args: &Value, _ctx: &ToolContext) -> Option<String> {
        None
    }
}

/// The default tool set.
pub fn default_registry() -> Registry {
    Registry {
        tools: vec![
            Box::new(ReadTool),
            Box::new(WriteTool),
            Box::new(EditTool),
            Box::new(LsTool),
            Box::new(GlobTool),
            Box::new(GrepTool),
            Box::new(BashTool),
            Box::new(TodoWriteTool),
        ],
    }
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec::function(t.name(), t.description(), t.parameters()))
            .collect()
    }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|b| b.as_ref())
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}

// ---------- path jail + arg helpers ----------

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `p` against the workspace and ensure it does not escape it (lexically).
pub fn jail(ws: &Path, p: &str) -> Result<PathBuf> {
    let raw = Path::new(p);
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { ws.join(raw) };
    let norm = normalize(&joined);
    let wsn = normalize(ws);
    if !norm.starts_with(&wsn) {
        bail!("path '{p}' escapes the workspace ({})", ws.display());
    }
    Ok(norm)
}

fn rel(ws: &Path, p: &Path) -> String {
    p.strip_prefix(ws).unwrap_or(p).display().to_string()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing required string argument '{key}'"))
}

fn arg_str_opt<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn arg_u64_opt(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

// ---------- Read ----------

struct ReadTool;
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a UTF-8 text file from the workspace. Optionally start at a 1-based line `offset` and read up to `limit` lines. Output is line-numbered."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to the workspace"},
                "offset": {"type": "integer", "description": "1-based starting line"},
                "limit": {"type": "integer", "description": "Max lines to read"}
            },
            "required": ["path"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("read {}", arg_str_opt(args, "path").unwrap_or("?"))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = jail(&ctx.workspace, arg_str(args, "path")?)?;
        let meta = std::fs::metadata(&path)
            .map_err(|e| anyhow!("cannot stat {}: {e}", path.display()))?;
        if meta.len() > MAX_READ_BYTES {
            return Ok(ToolOutput::err(format!(
                "file is {} bytes (> {MAX_READ_BYTES} limit); use grep or read a range",
                meta.len()
            )));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
        let offset = arg_u64_opt(args, "offset").unwrap_or(1).max(1) as usize;
        let limit = arg_u64_opt(args, "limit").map(|l| l as usize);
        let mut out = String::new();
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            if n < offset {
                continue;
            }
            if let Some(l) = limit {
                if n >= offset + l {
                    break;
                }
            }
            out.push_str(&format!("{n:>6}\t{line}\n"));
        }
        if out.is_empty() {
            out.push_str("(empty or no lines in range)");
        }
        Ok(ToolOutput::ok(out))
    }
}

// ---------- Write ----------

struct WriteTool;
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "Create or overwrite a file with the given content. Creates parent directories. Approval-gated."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        })
    }
    fn mutating(&self) -> bool {
        true
    }
    fn summary(&self, args: &Value) -> String {
        format!("write {}", arg_str_opt(args, "path").unwrap_or("?"))
    }
    fn preview(&self, args: &Value, ctx: &ToolContext) -> Option<String> {
        let path = jail(&ctx.workspace, arg_str_opt(args, "path")?).ok()?;
        let new = arg_str_opt(args, "content")?;
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        Some(crate::permissions::diff::render(&rel(&ctx.workspace, &path), &old, new))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = jail(&ctx.workspace, arg_str(args, "path")?)?;
        let content = arg_str(args, "content")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(ToolOutput::ok(format!(
            "wrote {} ({} bytes)",
            rel(&ctx.workspace, &path),
            content.len()
        )))
    }
}

// ---------- Edit (fail-loud search/replace) ----------

struct EditTool;
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> &'static str {
        "Replace an exact `old_string` with `new_string` in a file. Fails loudly if `old_string` is not found, or if it appears more than once (unless `replace_all` is true). Include enough surrounding context to make `old_string` unique."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)"}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    fn mutating(&self) -> bool {
        true
    }
    fn summary(&self, args: &Value) -> String {
        format!("edit {}", arg_str_opt(args, "path").unwrap_or("?"))
    }
    fn preview(&self, args: &Value, ctx: &ToolContext) -> Option<String> {
        let path = jail(&ctx.workspace, arg_str_opt(args, "path")?).ok()?;
        let old_file = std::fs::read_to_string(&path).ok()?;
        let (new_file, _) = apply_edit(
            &old_file,
            arg_str_opt(args, "old_string")?,
            arg_str_opt(args, "new_string").unwrap_or(""),
            args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false),
        )
        .ok()?;
        Some(crate::permissions::diff::render(&rel(&ctx.workspace, &path), &old_file, &new_file))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = jail(&ctx.workspace, arg_str(args, "path")?)?;
        let old_string = arg_str(args, "old_string")?;
        let new_string = arg_str(args, "new_string")?;
        let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        let file = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
        let (new_file, n) = apply_edit(&file, old_string, new_string, replace_all)?;
        std::fs::write(&path, &new_file)?;
        Ok(ToolOutput::ok(format!(
            "edited {} ({n} replacement{})",
            rel(&ctx.workspace, &path),
            if n == 1 { "" } else { "s" }
        )))
    }
}

/// Apply a fail-loud search/replace. Returns (new_content, num_replacements).
pub fn apply_edit(file: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize)> {
    if old.is_empty() {
        bail!("old_string must not be empty");
    }
    let count = file.matches(old).count();
    if count == 0 {
        bail!("old_string not found in file (no changes made). Re-read the file and copy the exact text.");
    }
    if count > 1 && !replace_all {
        let occ = describe_occurrences(file, old);
        bail!("old_string appears {count} times, so it is ambiguous. Include the preceding line (shown below) in old_string to target exactly ONE occurrence — e.g. put the `def`/declaration line first. Only set replace_all=true if you truly intend to change ALL {count} occurrences.{occ}");
    }
    let new_file = if replace_all {
        file.replace(old, new)
    } else {
        file.replacen(old, new, 1)
    };
    Ok((new_file, if replace_all { count } else { 1 }))
}

/// List each occurrence of `old` with its line number and nearest preceding
/// non-blank line, so an ambiguous edit can be disambiguated by the model.
fn describe_occurrences(file: &str, old: &str) -> String {
    let mut info = Vec::new();
    let mut start = 0usize;
    while let Some(rel) = file[start..].find(old) {
        let pos = start + rel;
        let line_no = file[..pos].matches('\n').count() + 1;
        let prev = file[..pos]
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        info.push(format!("  - line {line_no}, directly below: `{prev}`"));
        start = pos + old.len();
        if info.len() >= 8 {
            break;
        }
    }
    if info.is_empty() {
        String::new()
    } else {
        format!("\nOccurrences:\n{}", info.join("\n"))
    }
}

// ---------- Ls ----------

struct LsTool;
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }
    fn description(&self) -> &'static str {
        "List the entries of a directory in the workspace (directories suffixed with /)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Directory (default workspace root)"}},
            "required": []
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("ls {}", arg_str_opt(args, "path").unwrap_or("."))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = jail(&ctx.workspace, arg_str_opt(args, "path").unwrap_or("."))?;
        let mut entries: Vec<String> = Vec::new();
        for e in std::fs::read_dir(&path).map_err(|e| anyhow!("cannot read dir {}: {e}", path.display()))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                entries.push(format!("{name}/"));
            } else {
                entries.push(name);
            }
        }
        entries.sort();
        if entries.is_empty() {
            return Ok(ToolOutput::ok("(empty directory)"));
        }
        Ok(ToolOutput::ok(entries.join("\n")))
    }
}

// ---------- Glob ----------

struct GlobTool;
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }
    fn description(&self) -> &'static str {
        "Find files matching a glob pattern (e.g. `src/**/*.rs`), relative to the workspace."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"pattern": {"type": "string"}},
            "required": ["pattern"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("glob {}", arg_str_opt(args, "pattern").unwrap_or("?"))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = arg_str(args, "pattern")?;
        let abs = ctx.workspace.join(pattern);
        let abs_str = abs.to_string_lossy().to_string();
        let mut matches = Vec::new();
        for entry in glob::glob(&abs_str).map_err(|e| anyhow!("bad glob: {e}"))? {
            if let Ok(p) = entry {
                matches.push(rel(&ctx.workspace, &p));
            }
            if matches.len() >= 500 {
                matches.push("[truncated at 500]".to_string());
                break;
            }
        }
        if matches.is_empty() {
            return Ok(ToolOutput::ok("(no matches)"));
        }
        matches.sort();
        Ok(ToolOutput::ok(matches.join("\n")))
    }
}

// ---------- Grep ----------

struct GrepTool;
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn description(&self) -> &'static str {
        "Search file contents with a regular expression (gitignore-aware). Returns `path:line:text` matches."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Rust regex"},
                "path": {"type": "string", "description": "Subdirectory to search (default workspace root)"}
            },
            "required": ["pattern"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("grep /{}/", arg_str_opt(args, "pattern").unwrap_or("?"))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = arg_str(args, "pattern")?;
        let re = regex::Regex::new(pattern).map_err(|e| anyhow!("bad regex: {e}"))?;
        let root = jail(&ctx.workspace, arg_str_opt(args, "path").unwrap_or("."))?;
        let mut out = String::new();
        let mut count = 0usize;
        for result in ignore::WalkBuilder::new(&root).hidden(false).build() {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    let trimmed = line.trim_end();
                    let shown = if trimmed.len() > 200 { &trimmed[..200] } else { trimmed };
                    out.push_str(&format!("{}:{}:{}\n", rel(&ctx.workspace, path), i + 1, shown));
                    count += 1;
                    if count >= 200 {
                        out.push_str("[truncated at 200 matches]\n");
                        return Ok(ToolOutput::ok(out));
                    }
                }
            }
        }
        if out.is_empty() {
            return Ok(ToolOutput::ok("(no matches)"));
        }
        Ok(ToolOutput::ok(out))
    }
}

// ---------- Bash ----------

struct BashTool;
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }
    fn description(&self) -> &'static str {
        "Run a shell command from the workspace root and return combined stdout/stderr. Approval-gated. Use for builds, tests, git, etc."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "description": "Optional timeout in ms"}
            },
            "required": ["command"]
        })
    }
    fn mutating(&self) -> bool {
        true
    }
    fn summary(&self, args: &Value) -> String {
        let cmd = arg_str_opt(args, "command").unwrap_or("?");
        let one = cmd.lines().next().unwrap_or(cmd);
        format!("bash: {one}")
    }
    fn preview(&self, args: &Value, _ctx: &ToolContext) -> Option<String> {
        Some(format!("$ {}", arg_str_opt(args, "command")?))
    }
    fn run(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = arg_str(args, "command")?;
        let timeout = arg_u64_opt(args, "timeout_ms")
            .map(Duration::from_millis)
            .unwrap_or(ctx.bash_timeout);
        let (code, output, timed_out) = run_bash(command, &ctx.workspace, timeout)?;
        let mut content = output;
        if content.len() > 16_000 {
            let tail = &content[content.len() - 16_000..];
            content = format!("[output truncated to last 16000 bytes]\n{tail}");
        }
        if timed_out {
            return Ok(ToolOutput::err(format!(
                "command timed out after {timeout:?} and was killed.\n{content}"
            )));
        }
        let header = format!("[exit {code}]\n");
        Ok(ToolOutput {
            content: format!("{header}{content}"),
            is_error: code != 0,
        })
    }
}

/// Run a shell command with a hard timeout. Returns (exit_code, combined_output, timed_out).
fn run_bash(command: &str, cwd: &Path, timeout: Duration) -> Result<(i32, String, bool)> {
    use std::process::{Command, Stdio};
    use std::thread;

    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("failed to start shell: {e}"))?;

    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let th_o = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let th_e = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(40));
    };

    let mut out = th_o.join().unwrap_or_default();
    let err = th_e.join().unwrap_or_default();
    let mut combined = String::from_utf8_lossy(&out).into_owned();
    out.clear();
    let err_s = String::from_utf8_lossy(&err);
    if !err_s.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&err_s);
    }
    let code = status.and_then(|s| s.code()).unwrap_or(if timed_out { 124 } else { -1 });
    Ok((code, combined, timed_out))
}

// ---------- TodoWrite ----------

struct TodoWriteTool;
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todo_write"
    }
    fn description(&self) -> &'static str {
        "Record/replace your task list for the current multi-step job. Pass an array of {content, status} where status is one of pending|in_progress|completed."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, _args: &Value) -> String {
        "update todo list".to_string()
    }
    fn run(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let todos = args
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("missing 'todos' array"))?;
        let mut out = String::new();
        for t in todos {
            let content = t.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("pending");
            let mark = match status {
                "completed" => "[x]",
                "in_progress" => "[~]",
                _ => "[ ]",
            };
            out.push_str(&format!("{mark} {content}\n"));
        }
        Ok(ToolOutput::ok(out))
    }
}
