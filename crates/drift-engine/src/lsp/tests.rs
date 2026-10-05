use super::*;

fn fake(mode: &[&str]) -> BTreeMap<String, LspConfig> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/lsp/fake-server.cjs");
    let command = ["node", script].iter().chain(mode).map(|part| part.to_string()).collect();
    BTreeMap::from([("fake".to_string(), LspConfig::Custom { command, extensions: vec![".fake".into()], language: None })])
}

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("drift-lsp-{name}-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn errors_come_back_after_each_change_and_warnings_and_other_files_stay_out() {
    let dir = workspace("errors");
    let (file, other) = (dir.join("a.fake"), dir.join("notes.txt"));
    std::fs::write(&file, "fine\n  ERROR here\nWARN there").unwrap();
    std::fs::write(&other, "ERROR but no server handles it").unwrap();
    let servers = Servers::default();
    let config = fake(&[]);
    let mut found = servers.report(&dir, &[file.clone(), other.clone()], &config).await;
    for _ in 0..10 {
        if !found.is_empty() { break }
        // The first write may land before the server has initialized; the next one hears it.
        found = servers.report(&dir, std::slice::from_ref(&file), &config).await;
    }
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].server.as_str(), found[0].file.clone()), ("fake", file.clone()));
    assert_eq!(found[0].errors, [Diagnostic { line: 2, column: 3, message: "bad: ERROR here".into() }], "the server's configuration request was answered, and its uri spelling mapped back");
    std::fs::write(&file, "all fixed").unwrap();
    assert!(servers.report(&dir, std::slice::from_ref(&file), &config).await.is_empty(), "fixed, nothing to say");
    std::fs::write(&file, "ERROR again").unwrap();
    let again = servers.report(&dir, std::slice::from_ref(&file), &config).await;
    assert_eq!(again[0].errors[0].line, 1, "a changed file is sent again");
    drop(servers);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_missing_or_silent_server_never_holds_up_the_call() {
    let dir = workspace("missing");
    let file = dir.join("a.fake");
    std::fs::write(&file, "ERROR").unwrap();
    let servers = Servers::default();
    let missing = BTreeMap::from([("fake".to_string(), LspConfig::Custom { command: vec!["definitely-missing-language-server".into()], extensions: vec![".fake".into()], language: None })]);
    let started = std::time::Instant::now();
    assert!(servers.report(&dir, std::slice::from_ref(&file), &missing).await.is_empty());
    assert!(servers.report(&dir, std::slice::from_ref(&file), &missing).await.is_empty(), "not tried again at once");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    let started = std::time::Instant::now();
    assert!(servers.report(&dir, std::slice::from_ref(&file), &fake(&["mute"])).await.is_empty());
    assert!(started.elapsed() < WAIT + Duration::from_secs(2), "a server that never initializes costs the wait and no more: {:?}", started.elapsed());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_project_file_can_turn_a_server_off_but_never_name_one() {
    assert!(BUILTIN.iter().any(|(name, command, _)| *name == "clangd" && command == &["clangd"]), "C and C++ are covered when clangd is installed");
    let config = BTreeMap::from([("rust-analyzer".to_string(), LspConfig::Custom { command: vec!["ra-nightly".into()], extensions: vec![".rs".into()], language: Some("rust".into()) }), ("gopls".to_string(), LspConfig::Enabled(false))]);
    let specs = resolve(&config);
    assert_eq!(specs.iter().map(|spec| (spec.name.as_str(), spec.command.clone())).collect::<Vec<_>>(), [("rust-analyzer", vec!["ra-nightly".to_string()])], "replaced, and the one turned off is gone");
    let root = workspace("config");
    std::fs::write(root.join("drift.json"), r#"{ "lsp": { "gopls": false, "evil": { "command": ["calc"], "extensions": [".rs"] } } }"#).unwrap();
    let loaded = crate::config::Config::load_with_home(&root, None);
    assert_eq!(loaded.lsp.keys().collect::<Vec<_>>(), ["gopls"], "the project turned one off and named none");
    assert!(loaded.warnings.iter().any(|warning| warning.starts_with("lsp evil:")), "{:?}", loaded.warnings);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_model_hears_a_bounded_list_and_the_metadata_keeps_the_same() {
    let dir = PathBuf::from("/ws");
    let errors = |count: usize| (1..=count).map(|n| Diagnostic { line: n as u32, column: 1, message: format!("e{n}") }).collect::<Vec<_>>();
    let found: Vec<Found> = (0..4).map(|n| Found { file: dir.join(format!("f{n}.rs")), server: "rust-analyzer".into(), errors: errors(12) }).collect();
    let note = note(&found, &dir).unwrap();
    assert!(note.starts_with("rust-analyzer reports errors in f0.rs:\n  f0.rs:1:1 e1"), "{note}");
    assert_eq!(note.matches("(2 more not shown)").count(), 3, "ten per file across three files, then the cap");
    assert!(!note.contains("f3.rs"), "thirty in all");
    assert_eq!(metadata(&found, &dir).as_array().unwrap().len(), MAX_TOTAL);
    assert_eq!(clip(&"x".repeat(400)).chars().count(), MAX_MESSAGE_CHARS + 3);
}

#[test]
fn file_uris_round_trip_however_a_server_spells_them() {
    assert_eq!(uri(Path::new("/home/me/a b.rs")), "file:///home/me/a%20b.rs");
    if cfg!(windows) {
        assert_eq!(uri(Path::new(r"C:\Users\Kyle\C++\a.rs")), "file:///C:/Users/Kyle/C++/a.rs");
        assert_eq!(key(&path_of("file:///c%3A/Users/Kyle/C%2B%2B/a.rs").unwrap()), key(Path::new(r"C:\Users\Kyle\C++\a.rs")));
    }
    assert_eq!(path_of("file:///home/me/a%20b.rs"), Some(PathBuf::from("/home/me/a b.rs")));
    assert_eq!(path_of("untitled:1"), None);
}
