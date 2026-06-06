//! First-run onboarding wizard (permission prompt → hardware scan → model pick →
//! download → config). The full inquire-driven flow is implemented in M3; this
//! module currently exposes the trigger check used by the CLI.

/// True when no config exists yet (first run → onboarding should run).
pub fn first_run() -> bool {
    !crate::config::config_path().exists()
}
