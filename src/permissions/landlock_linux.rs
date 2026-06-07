//! Linux network confinement for the bash sandbox, via the Landlock LSM.
//!
//! This is the Linux counterpart to the macOS Seatbelt profile (`sandbox-exec`
//! `(deny network*)`): it denies all TCP to a spawned shell while leaving the
//! filesystem to the userland path-jail + approval gate — so `cargo`/`npm`/`pip`
//! (which write to home caches outside the workspace) keep working, exactly as on
//! macOS. Landlock's network rules need ABI V4 (Linux 6.7); on older kernels the
//! BestEffort policy degrades to a no-op and the child simply runs unconfined.
//!
//! Grounded in rust-landlock 0.4.5: the entire ruleset (which opens FDs / allocates)
//! is built in the PARENT, then moved into a `pre_exec` closure that calls ONLY
//! `restrict_self()` — a couple of async-signal-safe syscalls (prctl +
//! landlock_restrict_self) — which is the only landlock work that is safe after fork.

#![allow(unused_imports)]

use landlock::{
    Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, Ruleset, RulesetAttr,
    RulesetCreated, RulesetCreatedAttr, RulesetStatus, ABI,
};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Build a ruleset that governs network access but grants NO ports → all TCP
/// bind/connect denied. Filesystem is intentionally left unrestricted (handled by
/// the path-jail), matching the macOS network-only policy.
fn build_net_deny() -> Result<RulesetCreated, landlock::RulesetError> {
    let abi = ABI::V4; // first ABI with TCP bind/connect rights
    Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessNet::from_all(abi))?
        .create()
    // No add_rules(NetPort...) → no port is allowed → all TCP is denied.
}

/// Attach network-denying Landlock confinement to `cmd` (applied in the child).
/// Best-effort: if a ruleset can't be built (old kernel, no Landlock), the child
/// runs unconfined and we leave a note — never a hard failure.
pub fn deny_network(cmd: &mut Command) {
    let created = match build_net_deny() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("localcode: landlock unavailable, bash network not OS-enforced ({e})");
            return;
        }
    };
    // RulesetCreated is move-once; pre_exec is FnMut → take() it on first call.
    let mut slot = Some(created);
    // SAFETY: the closure runs in the child between fork() and exec(). It only calls
    // restrict_self() (prctl + landlock_restrict_self — both async-signal-safe). All
    // FD-opening/allocation already happened in build_net_deny() in the parent. We
    // return a raw-errno io::Error (no allocation) on failure, per the std safety docs.
    unsafe {
        cmd.pre_exec(move || {
            if let Some(rs) = slot.take() {
                rs.restrict_self()
                    .map_err(|_| std::io::Error::from_raw_os_error(libc::EPERM))?;
            }
            Ok(())
        });
    }
}
