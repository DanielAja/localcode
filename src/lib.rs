//! localcode — a 100% local, on-device CLI AI coding agent.
//!
//! The crate is split into focused modules; see `src/` and the plan in
//! `~/.claude/plans/ultrathink-deep-adaptive-goblet.md` for the architecture.

pub mod agent;
pub mod cli;
pub mod commands;
pub mod config;
pub mod engine;
pub mod eval;
pub mod hardware;
pub mod models;
pub mod onboarding;
pub mod permissions;
pub mod session;
pub mod toolcall;
pub mod tools;
pub mod ui;

/// Crate-wide result alias.
pub type Result<T> = anyhow::Result<T>;
