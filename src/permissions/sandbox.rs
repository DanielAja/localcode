//! OS-level sandboxing for shell commands.
//!
//! macOS: Seatbelt (`sandbox-exec`) denies network at the read-only/workspace-write
//! levels — enforcing "network off by default" while still allowing local file and
//! build tools. `full` runs unconfined.
//!
//! Linux: Landlock (best-effort, ABI V4+/Linux 6.7) denies network to the shell —
//! the same network-only policy as macOS — leaving the filesystem to the path-jail
//! so build caches in `$HOME` keep working. Degrades cleanly on older kernels.
//!
//! Windows: OS-level FS isolation (AppContainer) is high-complexity and breaks many
//! dev tools, so we deliberately rely on the userland path-jail + approval gate
//! there. The workspace path-jail on file tools applies on every platform.

use crate::config::SandboxLevel;
use std::process::Command;

/// Build the (possibly sandboxed) shell `Command` for a bash invocation.
/// The caller still sets cwd/stdio.
pub fn bash_command(level: SandboxLevel, command: &str) -> Command {
    build(level, command)
}

#[cfg(target_os = "macos")]
fn build(level: SandboxLevel, command: &str) -> Command {
    if matches!(level, SandboxLevel::Full) {
        return plain_sh(command);
    }
    // Allow everything except network. This keeps `cargo build`, `pytest`, `git`
    // (local), etc. working while blocking exfiltration / remote fetches.
    const PROFILE: &str = "(version 1)(allow default)(deny network*)";
    let mut c = Command::new("/usr/bin/sandbox-exec");
    c.arg("-p").arg(PROFILE).arg("/bin/sh").arg("-c").arg(command);
    c
}

#[cfg(target_os = "linux")]
fn build(level: SandboxLevel, command: &str) -> Command {
    let mut c = plain_sh(command);
    if !matches!(level, SandboxLevel::Full) {
        // Deny network via Landlock (best-effort), matching the macOS policy.
        super::landlock_linux::deny_network(&mut c);
    }
    c
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn build(_level: SandboxLevel, command: &str) -> Command {
    // Other unix (BSD, etc.): no OS sandbox yet; path-jail + approval gate apply.
    plain_sh(command)
}

#[cfg(unix)]
fn plain_sh(command: &str) -> Command {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(command);
    c
}

#[cfg(windows)]
fn build(_level: SandboxLevel, command: &str) -> Command {
    // TODO(roadmap): AppContainer / Job Objects. Default stays read-only on Windows.
    let mut c = Command::new("cmd");
    c.arg("/C").arg(command);
    c
}

/// Whether OS-level network denial is enforced for bash on this platform/level.
/// (Linux Landlock is best-effort — true here means "attempted/enforced where the
/// kernel supports it", ABI V4+/Linux 6.7.)
pub fn network_enforced(level: SandboxLevel) -> bool {
    (cfg!(target_os = "macos") || cfg!(target_os = "linux")) && !matches!(level, SandboxLevel::Full)
}

/// Human-readable name of the active OS sandbox backend (for `doctor`).
pub fn backend_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Seatbelt"
    } else if cfg!(target_os = "linux") {
        "Landlock (best-effort, needs Linux 6.7+)"
    } else {
        "path-jail only"
    }
}
