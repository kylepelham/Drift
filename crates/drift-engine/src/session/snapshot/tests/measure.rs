use super::*;

/// `cargo test -p drift-engine --release -- --ignored capture_cost --nocapture`; set
/// `DRIFT_MEASURE_WORKSPACE` to time a real tree instead of 5,000 generated files.
#[tokio::test]
#[ignore]
async fn capture_cost() {
    let (base, generated) = dirs();
    for index in 0..5_000 {
        let dir = generated.join(format!("d{}", index % 50));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("f{index}.txt")), vec![b'a' + (index % 26) as u8; 1024]).unwrap();
    }
    let workspace = std::env::var("DRIFT_MEASURE_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(generated.clone());
    let snapshots = Snapshots::new(&base.join("data"));
    let time = |label: &'static str, started: std::time::Instant| eprintln!("{label}: {:?}", started.elapsed());
    let started = std::time::Instant::now();
    snapshots.take(&workspace).await.unwrap();
    time("first capture", started);
    for round in 0..3 {
        let started = std::time::Instant::now();
        snapshots.take(&workspace).await.unwrap();
        time(
            ["unchanged capture 1", "unchanged capture 2", "unchanged capture 3"][round],
            started,
        );
    }

    let started = std::time::Instant::now();
    let root = workspace.clone();
    tokio::task::spawn_blocking(move || large_files(&root, MAX_TREE_FILES))
        .await
        .unwrap();
    time("size walk alone", started);
    std::fs::remove_dir_all(base).ok();
}
