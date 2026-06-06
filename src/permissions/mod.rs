//! Permission policy: which tool calls run automatically, which need approval.

pub mod diff;
pub mod sandbox;

use crate::config::SandboxLevel;
use crate::tools::Tool;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Run without asking.
    Allow,
    /// Ask the user (with a preview).
    Ask,
    /// Refuse (e.g. a mutating tool under read-only).
    Deny,
}

pub struct Policy {
    pub level: SandboxLevel,
    /// Tool names the user approved "always" for this session.
    allow: HashSet<String>,
    /// Auto-approve everything (non-interactive `--yes` / dangerous).
    pub yolo: bool,
}

impl Policy {
    pub fn new(level: SandboxLevel) -> Self {
        Policy {
            level,
            allow: HashSet::new(),
            yolo: false,
        }
    }

    pub fn decide(&self, tool: &dyn Tool) -> Decision {
        if self.yolo {
            return Decision::Allow;
        }
        match self.level {
            SandboxLevel::ReadOnly => {
                if tool.mutating() {
                    Decision::Deny
                } else {
                    Decision::Allow
                }
            }
            SandboxLevel::WorkspaceWrite => {
                if !tool.mutating() || self.allow.contains(tool.name()) {
                    Decision::Allow
                } else {
                    Decision::Ask
                }
            }
            SandboxLevel::Full => Decision::Allow,
        }
    }

    /// Approve a tool for the rest of the session.
    pub fn allow_for_session(&mut self, tool_name: &str) {
        self.allow.insert(tool_name.to_string());
    }
}
