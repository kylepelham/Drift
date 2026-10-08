use super::*;

#[test]
fn project_config_layers_over_home_and_discovers_everything() {
    let root = std::env::temp_dir().join(format!("drift-config-{}", crate::random_hex(4)));
    let home = root.join("home");
    let workspace = root.join("ws");
    write_layered_home(&home);
    write_layered_workspace(&workspace);

    let config = Config::load_with_home(&workspace, Some(&home));
    assert_layered_policy(&config);
    assert_layered_agents(&config);
    assert_layered_commands_and_skills(&config);

    std::fs::remove_dir_all(root).ok();
}

fn write_layered_home(home: &Path) {
    write(
        home,
        ".config/drift/drift.json",
        r#"{ "model": { "provider": "anthropic", "model": "haiku" }, "permissions": [{ "kind": "bash", "pattern": "git *", "decision": "allow" }] }"#,
    );
    write(
        home,
        ".agents/skills/review/SKILL.md",
        "---\nname: review\ndescription: Reviews code\n---\nHow to review.",
    );
    write(
        home,
        ".claude/skills/team/lint/SKILL.md",
        "---\nname: lint\ndescription: Lints\n---\nLint it.",
    );
    write(
        home,
        ".agents/skills/notes/SKILL.md",
        "---\ndescription: Takes notes\n---\nWrite it down.",
    );
    write(
        home,
        "shared/skills/release/SKILL.md",
        "---\ndescription: Releases\n---\nTag it.",
    );
}

fn write_layered_workspace(workspace: &Path) {
    write(
        workspace,
        "drift.json",
        r#"{ "permissions": [{ "kind": "bash", "pattern": "git push*", "decision": "deny" }], "instructions": ["docs/rules.md"], "skillPaths": ["~/shared/skills"] }"#,
    );
    write(workspace, "docs/rules.md", "Be careful.");
    write(workspace, "AGENTS.md", "Repo rules.");
    write(workspace, "CLAUDE.md", "ignored when AGENTS.md exists");
    write(
        workspace,
        ".drift/agents/reviewer.md",
        "---\ndescription: Reviews PRs\nmode: subagent\nmodel: openai/gpt-5.5\ntools: read, grep\n---\nYou review.",
    );
    write(
        workspace,
        ".drift/agents/explore.md",
        "---\ndescription: Our explorer\n---\nSearch our monorepo.",
    );
    write(
        workspace,
        ".drift/agents/plan.md",
        "---\ndescription: My plan\n---\nCustom plan.",
    );
    write(
        workspace,
        ".drift/agents/title.md",
        "---\nmodel: openai/gpt-5-nano\n---\nShort titles.",
    );
    write(
        workspace,
        ".drift/commands/test.md",
        "---\ndescription: Run tests\n---\nRun the tests for $ARGUMENTS and report.",
    );
    write(
        workspace,
        ".drift/skills/review/SKILL.md",
        "---\nname: review\ndescription: Project review\n---\nProject way.",
    );
    write(
        workspace,
        ".claude/skills/deploy/SKILL.md",
        "---\ndescription: Deploys\n---\nShip it.",
    );
}

fn assert_layered_policy(config: &Config) {
    assert_eq!(
        config.model,
        Some(ModelRef {
            provider: "anthropic".into(),
            model: "haiku".into()
        })
    );
    assert_eq!(
        config
            .permissions
            .iter()
            .map(|rule| (rule.pattern.as_str(), rule.decision))
            .collect::<Vec<_>>(),
        [("git push*", Decision::Deny), ("git *", Decision::Allow)]
    );
    assert_eq!(
        config
            .policy()
            .decide(&crate::tool::Ask::new("bash", "git push origin", "")),
        Decision::Deny
    );
}

fn assert_layered_agents(config: &Config) {
    let names: Vec<&str> = config.agents.iter().map(|agent| agent.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "build",
            "general",
            "orchestrator",
            "compaction",
            "explore",
            "plan",
            "reviewer",
            "title"
        ]
    );
    assert_eq!(
        config.agent("reviewer").unwrap().kind,
        AgentKind::Subagent,
        "mode: subagent keeps it out of the composer"
    );
    assert_eq!(
        config.agent("explore").unwrap().kind,
        AgentKind::Subagent,
        "replacing a subagent without a mode keeps its kind"
    );
    assert_eq!(config.agent("plan").unwrap().kind, AgentKind::Primary);

    let title = config.agent("title").unwrap();
    assert_eq!(
        (title.kind, title.prompt.as_str()),
        (AgentKind::Action, "Short titles."),
        "a project file customises an action, it does not replace it"
    );
    assert_eq!(
        config.agent_model("title"),
        Some(ModelRef {
            provider: "openai".into(),
            model: "gpt-5-nano".into()
        })
    );
    let reviewer = config.agent("reviewer").unwrap();
    assert_eq!(reviewer.tools, ["read", "grep"]);
    assert_eq!(
        reviewer.model,
        Some(ModelRef {
            provider: "openai".into(),
            model: "gpt-5.5".into()
        })
    );
    assert_eq!(reviewer.prompt, "You review.");
    assert!(
        !config.agent("plan").unwrap().builtin,
        "a project agent replaces the built-in of the same name"
    );
}

fn assert_layered_commands_and_skills(config: &Config) {
    assert_eq!(config.commands[0].name, "test");
    assert!(config.commands[0].template.contains("$ARGUMENTS"));

    let skills: Vec<(&str, &str)> = config
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill.description.as_str()))
        .collect();
    assert_eq!(
        skills,
        [
            ("review", "Project review"),
            ("deploy", "Deploys"),
            ("release", "Releases"),
            ("notes", "Takes notes"),
            ("lint", "Lints")
        ],
        "project skills shadow home skills of the same name; home ones come from ~/.agents and ~/.claude at any depth, and listed paths are searched too"
    );
    assert_eq!(
        config
            .instructions
            .iter()
            .map(|instruction| instruction.name.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md", "docs/rules.md"]
    );
}
