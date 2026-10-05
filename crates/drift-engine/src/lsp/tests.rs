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
    let named = |name: &str| table::BUILTIN.iter().find(|server| server.name == name);
    assert!(named("clangd").is_some() && named("csharp").is_some_and(|server| server.extensions.contains(&".cs")), "C, C++ and C# are covered when installed");
    let config = BTreeMap::from([("rust-analyzer".to_string(), LspConfig::Custom { command: vec!["ra-nightly".into()], extensions: vec![".rs".into()], language: Some("rust".into()) }), ("gopls".to_string(), LspConfig::Enabled(false))]);
    let specs = resolve(&config);
    assert_eq!(specs.iter().map(|spec| (spec.name.as_str(), spec.commands.clone())).collect::<Vec<_>>(), [("rust-analyzer", vec![vec!["ra-nightly".to_string()]])], "replaced, and the one turned off is gone");
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

#[test]
fn servers_root_at_the_nearest_project_not_the_workspace() {
    use table::{root_for, Root};
    let ws = workspace("roots");
    let put = |path: &str, text: &str| {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    put("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]");
    put("crates/a/Cargo.toml", "[package]");
    put("crates/a/src/lib.rs", "");
    put("tools/solo/Cargo.toml", "[package]");
    put("tools/solo/src/main.rs", "");
    put("packages/web/package-lock.json", "{}");
    put("packages/web/src/app.ts", "");
    put("edge/deno.json", "{}");
    put("edge/main.ts", "");
    put("svc/Api.csproj", "<Project/>");
    put("svc/Program.cs", "");
    put("loose.java", "");
    let rule = |name: &str| table::BUILTIN.iter().find(|server| server.name == name).unwrap();
    let root = |name: &str, file: &str| root_for(&rule(name).root, rule(name).unless, &ws.join(file), &ws);
    assert_eq!(root("rust-analyzer", "crates/a/src/lib.rs"), Some(ws.clone()), "a member crate opens the Cargo workspace");
    assert_eq!(root("rust-analyzer", "tools/solo/src/main.rs"), Some(ws.clone()), "a crate under a Cargo workspace folder still opens it, as opencode does");
    assert_eq!(root("typescript", "packages/web/src/app.ts"), Some(ws.join("packages/web")), "a monorepo package gets its own server");
    assert_eq!(root("typescript", "edge/main.ts"), None, "Deno's project is Deno's");
    assert_eq!(root("deno", "edge/main.ts"), Some(ws.join("edge")));
    assert_eq!(root("deno", "packages/web/src/app.ts"), None, "Deno only under its own config");
    assert_eq!(root("csharp", "svc/Program.cs"), Some(ws.join("svc")), "*.csproj names any project file");
    assert_eq!(root("java", "loose.java"), None, "jdtls needs a build file");
    assert_eq!(root_for(&Root::Nearest(&[]), &[], &ws.join("svc/Program.cs"), &ws), Some(ws.clone()));
    std::fs::remove_dir_all(ws).ok();
}

#[test]
fn a_project_installed_server_is_found_in_node_modules() {
    let ws = workspace("node-bin");
    let bin = ws.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(ws.join("packages/web")).unwrap();
    let name = format!("drift-test-ls-{}", crate::random_hex(3));
    std::fs::write(bin.join(if cfg!(windows) { format!("{name}.cmd") } else { name.clone() }), "").unwrap();
    let found = table::installed(&[vec!["definitely-missing-ls".into()], vec![name.clone(), "--stdio".into()]], &ws.join("packages/web"), &ws);
    let (program, args) = found.expect("found below the workspace");
    assert!(program.starts_with(&bin) && args == ["--stdio"], "{program:?} {args:?}");
    assert!(table::installed(&[vec![name]], ws.parent().unwrap(), &ws).is_none(), "never above the workspace");
    std::fs::remove_dir_all(ws).ok();
}

#[tokio::test]
async fn reading_a_file_starts_its_server_before_the_first_edit() {
    let dir = workspace("warm");
    let file = dir.join("a.fake");
    std::fs::write(&file, "ERROR first").unwrap();
    let servers = Servers::default();
    let config = fake(&[]);
    servers.warm(&dir, &file, &config).await;
    assert_eq!(servers.running.lock().unwrap().len(), 1, "started by the read, before any write");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let found = servers.report(&dir, std::slice::from_ref(&file), &config).await;
    assert_eq!(found.len(), 1, "the first edit hears back: {found:?}");
    drop(servers);
    std::fs::remove_dir_all(dir).ok();
}