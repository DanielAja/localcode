//! Colorized unified-diff rendering for edit/write approval previews.

use crate::ui::style::{BOLD, GREEN, GREY, RED, RESET};
use similar::{ChangeTag, TextDiff};

/// Render a compact, colorized unified diff (3 lines of context per hunk).
pub fn render(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return format!("{BOLD}{path}{RESET}\n(no changes)");
    }
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    out.push_str(&format!("{BOLD}{path}{RESET}\n"));

    let groups = diff.grouped_ops(3);
    if groups.is_empty() {
        out.push_str("(no textual changes)");
        return out;
    }
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            out.push_str(&format!("{GREY}  ⋮{RESET}\n"));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let (sign, color) = match change.tag() {
                    ChangeTag::Delete => ("-", RED),
                    ChangeTag::Insert => ("+", GREEN),
                    ChangeTag::Equal => (" ", GREY),
                };
                let val = change.value();
                let val = val.strip_suffix('\n').unwrap_or(val);
                out.push_str(&format!("{color}{sign} {val}{RESET}\n"));
            }
        }
    }
    out
}
