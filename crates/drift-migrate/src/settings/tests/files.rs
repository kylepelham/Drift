use super::*;

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn prepare_files(opencode: &Path, home: &Path) {
    write(opencode.join("AGENTS.md"), "Be brief.");
    write(
        opencode.join("agents/reviewer.md"),
        "---\ndescription: Reviews\nmode: subagent\n---\nReview.",
    );
    write(
        opencode.join("command/ship.md"),
        "---\ndescription: Ship it\n---\nShip $ARGUMENTS.",
    );
    write(
        opencode.join("skills/impeccable/SKILL.md"),
        "---\nname: impeccable\ndescription: Design\n---\nDesign well.",
    );
    write(opencode.join("skills/impeccable/reference/colour.md"), "notes");
    write(
        opencode.join("skills/unslop/SKILL.md"),
        "---\nname: unslop\ndescription: Mine\n---\nTheirs.",
    );
    write(opencode.join("plugins/gk-hooks.js"), "export default {}");
    write(
        home.join(".config/drift/skills/unslop/SKILL.md"),
        "---\nname: unslop\ndescription: Mine\n---\nMine.",
    );
}

#[test]
fn opencodes_instructions_agents_commands_and_skills_are_copied_once_and_drift_reads_them() {
    let fixture = Fixture::new();
    let opencode = fixture.dir.0.join("opencode");
    let home = fixture.dir.0.join("home");
    prepare_files(&opencode, &home);
    let settings = Settings {
        auth: None,
        config: None,
        config_dir: opencode,
        servers: vec![],
    };

    let report = fixture.import(&home, &settings);
    let mut files = report.files.clone();
    files.sort();

    assert_eq!(
        files,
        [
            "AGENTS.md",
            "agents/reviewer.md",
            "commands/ship.md",
            "skills/impeccable/SKILL.md",
            "skills/impeccable/reference/colour.md",
        ]
    );
    let log = report.skipped.join("\n");
    assert!(
        log.contains("skills/unslop/SKILL.md: you already have one") && log.contains("plugin gk-hooks.js"),
        "{log}"
    );
    assert!(
        std::fs::read_to_string(home.join(".config/drift/skills/unslop/SKILL.md"))
            .unwrap()
            .contains("Mine."),
        "the user's own copy wins"
    );
    assert_eq!(
        (report.left_out.plugins, report.left_out.failed.len()),
        (vec!["gk-hooks.js".to_string()], 0)
    );

    let workspace = fixture.dir.0.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let config = drift_engine::config::Config::load_with_home(&workspace, Some(&home));

    assert!(
        config.agent("reviewer").is_some()
            && config.commands.iter().any(|command| command.name == "ship")
            && config.skill("impeccable").is_some()
    );
    assert!(
        config
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("Be brief.")),
        "the global instructions apply"
    );

    std::fs::remove_file(home.join(".config/drift/agents/reviewer.md")).unwrap();
    let again = fixture.import(&home, &settings);

    assert!(
        again.files.is_empty() && !home.join(".config/drift/agents/reviewer.md").exists(),
        "a copied file the user deleted stays deleted"
    );
}
