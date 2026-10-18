use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::json;

use super::servers::{self, Vars};
use super::*;
use crate::mcp::ServerConfig;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("drift-bundle-{name}-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

/// A plugin folder laid out as Claude Code reads one, with every kind of part.
fn plugin(root: &Path) {
    write(
        root,
        "skills/recon/SKILL.md",
        "---\nname: game-recon\ndescription: Finds the engine.\n---\nRun ${CLAUDE_PLUGIN_ROOT}/bin/um scan.",
    );
    write(root, "skills/nested/deep/SKILL.md", "no front matter");
    write(root, "extra/one/SKILL.md", "---\nname: one\n---\n");
    write(
        root,
        "agents/orchestrator.md",
        "---\nname: mod-orchestrator\ndescription: Runs workers.\n---\nPlan.",
    );
    write(root, "agents/worker.md", "---\ndescription: Works.\n---\nWork.");
    write(root, "commands/ship.md", "---\ndescription: Ships it.\n---\nShip $1.");
    write(root, "bin/um", "#!/usr/bin/env bash\n");
    write(root, "bin/um.cmd", "@echo off\n");
    write(
        root,
        ".mcp.json",
        &json!({ "mcpServers": { "fal": { "type": "http", "url": "https://mcp.fal.ai/mcp", "headers": { "Authorization": "Bearer ${FAL_KEY}" } } } }).to_string(),
    );
    write(
        root,
        ".claude-plugin/plugin.json",
        &json!({
            "name": "kit",
            "skills": ["./extra", "../escape"],
            "mcpServers": { "local": { "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"] } }
        })
        .to_string(),
    );
    write(
        root,
        "hooks/hooks.json",
        &json!({ "hooks": { "SessionStart": [], "PostToolUse": [] } }).to_string(),
    );
}

#[test]
fn a_plugin_folder_reads_as_claude_code_lays_it_out() {
    let root = temp("layout");
    plugin(&root);
    write(&root.join(".."), "escape/x/SKILL.md", "outside");

    let layout = Layout::read(&root);
    assert_eq!(
        names(&layout.skills),
        ["deep", "game-recon", "one"],
        "front matter names a skill, else its folder; the manifest adds a folder but never one outside"
    );
    assert_eq!(layout.skills[1].description, "Finds the engine.");
    assert_eq!(names(&layout.agents), ["mod-orchestrator", "worker"]);
    assert_eq!(names(&layout.commands), ["ship"]);
    assert_eq!(layout.servers.keys().collect::<Vec<_>>(), ["fal", "local"]);
    assert_eq!(layout.programs, ["um", "um.cmd"]);
    assert_eq!(layout.hooks, ["PostToolUse", "SessionStart"]);

    let lone = temp("lone");
    write(&lone, "SKILL.md", "---\nname: lone\n---\nAll of it.");
    assert_eq!(
        names(&Layout::read(&lone).skills),
        ["lone"],
        "a plugin that is one skill"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&lone);
}

#[test]
fn server_definitions_take_the_bundles_paths_and_the_values_the_user_gave() {
    let values = BTreeMap::from([("FAL_KEY".to_owned(), "k1".to_owned())]);
    let vars = Vars {
        root: "C:/drift/bundles/kit".into(),
        data: "C:/drift/bundle-data/kit".into(),
        values: &values,
    };
    assert_eq!(
        servers::expand("Bearer ${FAL_KEY}", &vars).as_deref(),
        Some("Bearer k1")
    );
    assert_eq!(servers::expand("${PORT:-8080}/x", &vars).as_deref(), Some("8080/x"));
    assert_eq!(servers::expand("${MISSING}", &vars), None);

    let remote = json!({ "type": "http", "url": "https://x.test/mcp", "headers": { "Authorization": "Bearer ${FAL_KEY}", "X-Org": "${ORG}" } });
    let Some(ServerConfig::Http { headers, .. }) = servers::config(&remote, &vars) else {
        panic!("an http server")
    };
    assert_eq!(
        headers,
        BTreeMap::from([("Authorization".into(), "Bearer k1".into())]),
        "a header naming a value nobody gave is left out"
    );

    let local = json!({ "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"] });
    let Some(ServerConfig::Stdio { args, env, .. }) = servers::config(&local, &vars) else {
        panic!("a stdio server")
    };
    assert_eq!(args, ["C:/drift/bundles/kit/server.js"]);
    assert_eq!(env["CLAUDE_PLUGIN_ROOT"], "C:/drift/bundles/kit");
    assert!(servers::config(&json!({ "type": "ws", "url": "wss://x.test" }), &vars).is_none());

    assert_eq!(servers::row_name("fal", "kit", |_| false), "fal");
    assert_eq!(
        servers::row_name("fal", "kit", |name| name == "fal"),
        "kit-fal",
        "a name the user's own server has"
    );
    assert_eq!(servers::row_name("my server", "kit", |_| false), "my-server");
}

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(if path.contains("/bin/") { 0o755 } else { 0o644 });
        header.set_cksum();
        builder.append_data(&mut header, path, text.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn a_bundle_unpacks_only_its_folder_moved_to_the_top() {
    let into = temp("unpack");
    let bytes = tarball(&[
        ("repo-abc/README.md", "# repo"),
        ("repo-abc/plugins/kit/skills/a/SKILL.md", "a"),
        ("repo-abc/plugins/kit/bin/tool", "#!/bin/sh"),
        ("repo-abc/plugins/other/skills/b/SKILL.md", "b"),
    ]);

    assert_eq!(unpack(&bytes, &into, Path::new("plugins/kit")).unwrap(), 2);
    assert!(into.join("skills/a/SKILL.md").is_file() && into.join("bin/tool").is_file());
    assert!(!into.join("README.md").exists() && !into.join("plugins").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(into.join("bin/tool")).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "a program stays executable");
    }

    assert_eq!(
        unpack(&bytes, &into, Path::new("")).unwrap(),
        4,
        "an empty folder keeps everything"
    );
    assert_eq!(unpack(&bytes, &into, Path::new("missing")).unwrap(), 0);
    let _ = std::fs::remove_dir_all(&into);
}

fn active(root: &Path, off: &[&str]) -> Active {
    Active {
        root: root.to_path_buf(),
        data: root.join("data"),
        layout: Layout::read(root),
        state: State {
            off: false,
            parts_off: off.iter().map(ToString::to_string).collect::<BTreeSet<_>>(),
        },
    }
}

#[test]
fn a_workspace_gets_a_bundles_parts_that_are_on_beneath_its_own_and_the_users() {
    let root = temp("config");
    plugin(&root);
    let home = temp("home");
    let workspace = temp("ws");
    write(
        &home,
        ".config/drift/agents/worker.md",
        "---\ndescription: Mine.\n---\nMy worker.",
    );
    write(
        &home,
        ".config/drift/skills/one/SKILL.md",
        "---\nname: one\ndescription: Mine.\n---\n",
    );

    let bundle = active(&root, &["agent:mod-orchestrator", "skill:deep"]);
    let config = super::super::Config::load_from(&workspace, Some(&home), &[], std::slice::from_ref(&bundle));
    assert!(config.agent("mod-orchestrator").is_none(), "an agent switched off");
    assert_eq!(
        config.agent("worker").unwrap().prompt,
        "My worker.",
        "the user's own wins"
    );
    assert!(config.commands.iter().any(|command| command.name == "ship"));
    assert!(config.skill("deep").is_none(), "a skill switched off");
    assert_eq!(config.skill("one").unwrap().description, "Mine.");
    let recon = config.skill("game-recon").unwrap();
    assert_eq!(
        recon.instructions,
        format!("Run {}/bin/um scan.", root.to_string_lossy().replace('\\', "/")),
        "the bundle's own path is filled in"
    );
    assert_eq!(config.programs, [root.join("bin")]);

    let quiet = active(&root, &["programs"]);
    let config = super::super::Config::load_from(&workspace, Some(&home), &[], std::slice::from_ref(&quiet));
    assert!(config.programs.is_empty(), "programs switched off stay off the PATH");

    for dir in [root, home, workspace] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn engine() -> std::sync::Arc<crate::Engine> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = temp("engine");
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };

    crate::Engine::open_with(&directory, options).unwrap()
}

fn installed(id: &str) -> Installed {
    Installed {
        id: id.into(),
        name: "Kit".into(),
        version: "abc".into(),
        source: None,
        image: None,
        archive: String::new(),
        path: String::new(),
        installed_at: 0,
        servers: BTreeMap::new(),
    }
}

fn header(engine: &crate::Engine, name: &str) -> Option<String> {
    match engine.store.mcp_server(name).unwrap()?.config {
        ServerConfig::Http { headers, .. } => headers.get("Authorization").cloned(),
        _ => None,
    }
}

fn enabled(engine: &crate::Engine, name: &str) -> bool {
    engine.store.mcp_server(name).unwrap().unwrap().enabled
}

#[tokio::test]
async fn a_bundles_servers_follow_its_switches_and_keep_their_secrets_across_a_reinstall() {
    let engine = engine();
    let root = temp("servers");
    let data = root.join("data");
    let mine = ServerConfig::Stdio {
        command: "fal-cli".into(),
        args: vec![],
        env: Default::default(),
        cwd: None,
        timeout_seconds: None,
    };
    engine.store.save_mcp_server("fal", &mine).unwrap();
    engine.store.set_mcp_enabled("fal", false).unwrap();
    write(
        &root,
        ".mcp.json",
        &json!({
            "fal": { "type": "http", "url": "http://127.0.0.1:9/mcp", "headers": { "Authorization": "Bearer ${FAL_KEY}" } },
            "local": { "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/s.js"] }
        })
        .to_string(),
    );

    let mut kit = installed("kit");
    let values = BTreeMap::from([("FAL_KEY".to_owned(), "k1".to_owned())]);
    engine
        .save_bundle_servers(&mut kit, &values, &root, &data)
        .await
        .unwrap();
    assert_eq!(
        kit.servers,
        BTreeMap::from([("fal".into(), "kit-fal".into()), ("local".into(), "local".into())]),
        "the user's own fal keeps its name"
    );
    assert_eq!(header(&engine, "kit-fal").as_deref(), Some("Bearer k1"));
    assert_eq!(
        engine.store.mcp_server("kit-fal").unwrap().unwrap().bundle.as_deref(),
        Some("kit")
    );
    assert_eq!(
        engine.store.mcp_server("fal").unwrap().unwrap().config,
        mine,
        "never touched"
    );

    follows_its_switches(&engine, &kit).await;

    write(
        &root,
        ".mcp.json",
        &json!({ "fal": { "type": "http", "url": "http://127.0.0.1:9/mcp", "headers": { "Authorization": "Bearer ${FAL_KEY}" } } }).to_string(),
    );
    engine
        .save_bundle_servers(&mut kit, &BTreeMap::new(), &root, &data)
        .await
        .unwrap();
    assert_eq!(
        header(&engine, "kit-fal").as_deref(),
        Some("Bearer k1"),
        "an update keeps the key"
    );
    assert!(
        engine.store.mcp_server("local").unwrap().is_none(),
        "a server it no longer declares goes"
    );
    assert!(!enabled(&engine, "fal"), "the user's own switch is theirs");

    let _ = std::fs::remove_dir_all(&root);
}

async fn follows_its_switches(engine: &std::sync::Arc<crate::Engine>, kit: &Installed) {
    let state = engine
        .change_bundle_state("kit", |state| {
            state.parts_off.insert(part_key(BundlePartKind::Server, "local"));
        })
        .unwrap();
    engine.apply_bundle_servers(kit, &state).await.unwrap();
    assert!(!enabled(engine, "local") && enabled(engine, "kit-fal"));
    let state = engine.change_bundle_state("kit", |state| state.off = true).unwrap();
    engine.apply_bundle_servers(kit, &state).await.unwrap();
    assert!(
        !enabled(engine, "kit-fal"),
        "the bundle's switch turns every server off"
    );
    let state = engine.change_bundle_state("kit", |state| state.off = false).unwrap();
    engine.apply_bundle_servers(kit, &state).await.unwrap();
    assert!(
        enabled(engine, "kit-fal") && !enabled(engine, "local"),
        "back on, each follows its own"
    );
}
