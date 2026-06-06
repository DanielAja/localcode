//! Search/replace edits must fail loudly (never silently no-op or mis-edit).

use localcode::tools::apply_edit;

#[test]
fn missing_old_string_fails() {
    let err = apply_edit("abc\n", "xyz", "q", false).unwrap_err().to_string();
    assert!(err.contains("not found"), "got: {err}");
}

#[test]
fn ambiguous_without_replace_all_fails_with_occurrences() {
    let file = "def add():\n    return 0\n\ndef sub():\n    return 0\n";
    let err = apply_edit(file, "return 0", "return 1", false).unwrap_err().to_string();
    assert!(err.contains("appears 2 times"), "got: {err}");
    // The error should help disambiguate by listing occurrences.
    assert!(err.contains("Occurrences"), "got: {err}");
}

#[test]
fn unique_edit_succeeds() {
    let file = "def f():\n    return 1\n";
    let (out, n) = apply_edit(file, "    return 1", "    return 2", false).unwrap();
    assert_eq!(n, 1);
    assert!(out.contains("return 2") && !out.contains("return 1"));
}

#[test]
fn replace_all_changes_every_occurrence() {
    let (out, n) = apply_edit("a a a", "a", "b", true).unwrap();
    assert_eq!(n, 3);
    assert_eq!(out, "b b b");
}

#[test]
fn empty_old_string_is_rejected() {
    assert!(apply_edit("abc", "", "y", false).is_err());
}

#[test]
fn disambiguating_with_context_targets_one() {
    let file = "def add(a, b):\n    return a + b\n\n\ndef subtract(a, b):\n    return a + b\n";
    // Including the def line makes the match unique.
    let (out, n) = apply_edit(file, "def subtract(a, b):\n    return a + b", "def subtract(a, b):\n    return a - b", false).unwrap();
    assert_eq!(n, 1);
    assert!(out.contains("def add(a, b):\n    return a + b"), "add must be untouched");
    assert!(out.contains("def subtract(a, b):\n    return a - b"));
}
