//! OS-level sandboxing for shell commands.
//!
//! macOS: Seatbelt (`sandbox-exec`) denies network at the read-only/workspace-write
//! levels — enforcing "network off by default" while still allowing local file and
//! build tools. `full` runs unconfined.
//!
//! Linux/Windows: network-deny is not yet OS-enforced (roadmap: Landlock / Job
//! Objects). The workspace path-jail on file tools and the approval gate still apply.

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

#[cfg(all(unix, not(target_os = "macos")))]
fn build(_level: SandboxLevel, command: &str) -> Command {
    // TODO(roadmap): Landlock + a network namespace for true confinement.
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
pub fn network_enforced(level: SandboxLevel) -> bool {
    cfg!(target_os = "macos") && !matches!(level, SandboxLevel::Full)
}
