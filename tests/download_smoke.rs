//! Opt-in network test for the real download path (resume/stream/rename).
//! Run with: `cargo test --test download_smoke -- --ignored`

#[tokio::test]
#[ignore = "requires network"]
async fn downloads_a_small_file_for_real() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("README.md");
    let url = localcode::models::download::hf_url(
        "bartowski/Qwen2.5-Coder-7B-Instruct-GGUF",
        "README.md",
    );
    localcode::models::download::download(&url, &dest)
        .await
        .expect("download should succeed");
    let meta = std::fs::metadata(&dest).expect("file should exist");
    assert!(meta.len() > 0, "downloaded file should be non-empty");
    // Re-running is a no-op (already present).
    localcode::models::download::download(&url, &dest).await.unwrap();
}
