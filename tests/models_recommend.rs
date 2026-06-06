//! Model recommendation must respect both the RAM tier and free-disk gate.

use localcode::models;

#[test]
fn recommends_largest_fitting_model_for_16gb() {
    let m = models::recommend(16.0, 30.0).unwrap();
    assert_eq!(m.alias, "qwen2.5-coder-7b");
}

#[test]
fn falls_back_to_smaller_model_when_disk_is_tight() {
    // ~3 GB free: the 7B (4.7 GB) won't fit the +15% gate, the 3B (2.1 GB) will.
    let m = models::recommend(16.0, 3.0).unwrap();
    assert_eq!(m.alias, "qwen2.5-coder-3b");
}

#[test]
fn none_when_nothing_fits() {
    assert!(models::recommend(4.0, 1.0).is_none());
}

#[test]
fn by_alias_roundtrips() {
    assert_eq!(models::by_alias("qwen3-coder-30b-a3b").unwrap().min_ram_gb, 32.0);
    assert!(models::by_alias("nonexistent").is_none());
}
