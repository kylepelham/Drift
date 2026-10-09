use super::user::user_plugins_in;
use super::*;
use crate::permission::Decision;

mod layers;

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn plugins_come_from_the_users_file_and_stay_under_its_directory() {
    let root = std::env::temp_dir().join(format!("drift-config-plugins-{}", crate::random_hex(4)));
    let (home, workspace) = (root.join("home"), root.join("ws"));
    write(
        &home,
        ".config/drift/drift.json",
        r#"{ "plugins": [{ "path": "plugins/guard.wasm", "config": { "strict": true } }, "../escape.wasm", "C:/abs.wasm", "plugins/script.js"] }"#,
    );
    write(&workspace, "drift.json", r#"{ "plugins": ["theirs.wasm"] }"#);

    let listed = user_plugins_in(&home);
    assert_eq!(listed.len(), 4, "the project's file adds none");
    assert_eq!(
        listed[0].path,
        Ok(home.join(".config/drift").join("plugins/guard.wasm"))
    );
    assert_eq!(listed[0].config["strict"], true);
    assert_eq!(listed[1].config, serde_json::json!({}));
    assert!(
        listed[1]
            .path
            .as_ref()
            .is_err_and(|error| error.to_string().contains("stay under"))
    );
    assert!(listed[2].path.is_err());
    assert!(
        listed[3]
            .path
            .as_ref()
            .is_err_and(|error| error.to_string().contains(".wasm"))
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn only_commands_the_project_file_names_wait_for_the_users_say_so() {
    let root = std::env::temp_dir().join(format!("drift-config-trust-{}", crate::random_hex(4)));
    let (home, workspace) = (root.join("home"), root.join("ws"));
    write(
        &home,
        ".config/drift/drift.json",
        r#"{ "checks": { "mine": { "command": ["tsc"], "extensions": [".ts"] }, "shared": { "command": ["eslint", "$FILE"], "extensions": [".ts"] } } }"#,
    );
    write(
        &workspace,
        "drift.json",
        r#"{ "checks": { "shared": false, "theirs": { "command": ["make", "lint"], "extensions": [".c"] } }, "formatters": { "prettier": { "command": ["./fmt.sh", "$FILE"], "extensions": [".ts"] }, "rustfmt": false } }"#,
    );

    let config = Config::load_with_home(&workspace, Some(&home));
    assert_eq!(
        config.project_command_lines("check", &[workspace.join("main.C")]),
        ["check theirs: make lint"]
    );
    assert_eq!(
        config.project_command_lines("formatter", &[workspace.join("app.ts")]),
        ["formatter prettier: ./fmt.sh $FILE"]
    );
    assert!(
        config
            .project_command_lines("check", &[workspace.join("app.ts")])
            .is_empty(),
        "a check is asked about only for files it runs on"
    );
    assert!(
        config
            .project_command_lines("formatter", &[workspace.join("notes.md")])
            .is_empty(),
        "nothing of the project's would run on it, so nothing to ask"
    );

    let (formatters, checks) = config.only_allowed(|_| false);
    assert_eq!(
        checks.keys().collect::<Vec<_>>(),
        ["mine", "shared"],
        "the user's own run; a project's `false` still turns one off"
    );
    assert!(
        !formatters.contains_key("prettier") && formatters.contains_key("rustfmt"),
        "the built-in prettier comes back; a project's `false` stands"
    );
    let (formatters, checks) = config.only_allowed(|line| line.starts_with("check "));
    assert!(
        checks.contains_key("theirs") && !formatters.contains_key("prettier"),
        "each command is judged on its own"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn skill_commands_and_the_wrappers_that_call_them_offer_the_skills_choices() {
    let workspace = std::env::temp_dir().join(format!("drift-config-arguments-{}", crate::random_hex(4)));
    let skill = "---\nname: test-skill\ndescription: Skill description\nargument-hint: \"[audit|polish] [target]\"\n---\n| Command | Description |\n|---|---|\n| audit [target] | Check accessibility |\n| polish [target] | Final quality pass |";
    let template = r#"Call skill({ name: "test-skill" }) and follow its Commands section to handle $ARGUMENTS."#;
    write(&workspace, ".drift/skills/test-skill/SKILL.md", skill);
    write(
        &workspace,
        ".drift/skills/ordinary/SKILL.md",
        &skill.replace("name: test-skill", "name: ordinary"),
    );
    write(
        &workspace,
        ".drift/commands/design.md",
        &format!("---\ndescription: Wrapper\nagent: build\nsubtask: true\n---\n{template}"),
    );
    write(
        &workspace,
        ".drift/commands/ordinary.md",
        "---\ndescription: Ordinary command\n---\nDo ordinary work.",
    );

    let config = Config::load_with_home(&workspace, None);
    let command = |name: &str| config.commands.iter().find(|command| command.name == name).unwrap();
    let choices = |name: &str| {
        command(name)
            .subcommands
            .iter()
            .map(|subcommand| {
                (
                    subcommand.name.clone(),
                    subcommand.description.clone(),
                    subcommand.usage.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = vec![
        (
            "audit".to_string(),
            "Check accessibility".to_string(),
            Some("[target]".to_string()),
        ),
        ("polish".into(), "Final quality pass".into(), Some("[target]".into())),
    ];
    assert_eq!(
        (command("test-skill").usage.as_deref(), choices("test-skill")),
        (Some("[audit|polish] [target]"), expected.clone())
    );
    let design = command("design");
    assert_eq!(
        (design.template.as_str(), design.agent.as_deref(), design.subtask),
        (template, Some("build"), Some(true)),
        "the wrapper keeps its own settings"
    );
    assert_eq!(choices("design"), expected, "and offers the skill's choices");
    assert!(
        command("ordinary").subcommands.is_empty() && command("ordinary").usage.is_none(),
        "a same-name command that calls no skill inherits nothing"
    );

    std::fs::remove_dir_all(workspace).ok();
}

#[test]
fn agent_modes_hidden_disable_and_the_default_agent_read_as_opencode_reads_them() {
    let root = std::env::temp_dir().join(format!("drift-modes-{}", crate::random_hex(4)));
    let workspace = root.join("ws");
    write_agent_modes(&workspace);

    let config = Config::load_with_home(&workspace, None);
    assert_agent_modes(&config);
    assert_eq!(config.default_agent(), "lead");
    write(&workspace, "drift.json", r#"{ "defaultAgent": "quiet" }"#);
    assert_eq!(
        Config::load_with_home(&workspace, None).default_agent(),
        "build",
        "a subagent cannot run a conversation, so the default stands"
    );

    write(&workspace, ".drift/agents/build.md", "---\ndisable: true\n---\n");
    write(&workspace, ".drift/agents/compaction.md", "---\ndisable: true\n---\n");
    let without_build = Config::load_with_home(&workspace, None);
    assert_eq!(
        without_build.default_agent(),
        "plan",
        "build disabled: the first agent that runs conversations"
    );
    assert!(
        without_build
            .agent("compaction")
            .is_some_and(|agent| !agent.prompt.is_empty()),
        "the engine's own jobs keep their agents"
    );
    assert!(
        without_build
            .warnings
            .iter()
            .any(|warning| warning.starts_with("agent compaction: disable ignored")),
        "{:?}",
        without_build.warnings
    );

    std::fs::remove_dir_all(root).ok();
}

fn write_agent_modes(workspace: &Path) {
    write(
        workspace,
        ".drift/agents/helper.md",
        "---\ndescription: No mode\n---\nHelp.",
    );
    write(
        workspace,
        ".drift/agents/both.md",
        "---\ndescription: Both\nmode: all\n---\nBoth.",
    );
    write(
        workspace,
        ".drift/agents/lead.md",
        "---\ndescription: Lead\nmode: primary\n---\nLead.",
    );
    write(
        workspace,
        ".drift/agents/quiet.md",
        "---\ndescription: Internal\nmode: subagent\nhidden: true\n---\nQuiet.",
    );
    write(workspace, ".drift/agents/explore.md", "---\ndisable: true\n---\n");
    write(
        workspace,
        ".drift/agents/gone.md",
        "---\ndescription: Off\ndisable: true\n---\nNever.",
    );
    write(workspace, "drift.json", r#"{ "defaultAgent": "lead" }"#);
}

fn assert_agent_modes(config: &Config) {
    let kind = |name: &str| config.agent(name).map(|agent| agent.kind);
    assert_eq!(
        kind("helper"),
        Some(AgentKind::All),
        "no mode means both, as in opencode"
    );
    assert_eq!(
        (kind("both"), kind("lead"), kind("quiet")),
        (
            Some(AgentKind::All),
            Some(AgentKind::Primary),
            Some(AgentKind::Subagent)
        )
    );
    assert!(
        AgentKind::All.runs_conversations()
            && AgentKind::All.delegated_to()
            && !AgentKind::Primary.delegated_to()
            && !AgentKind::Subagent.runs_conversations()
    );
    assert!(config.agent("quiet").unwrap().hidden && !config.agent("helper").unwrap().hidden);
    assert!(
        config.agent("explore").is_none() && config.agent("gone").is_none(),
        "disable takes an agent away, a built-in included"
    );
}

#[test]
fn an_empty_workspace_still_has_the_builtin_agents() {
    let workspace = std::env::temp_dir().join(format!("drift-config-empty-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&workspace).unwrap();
    let config = Config::load_with_home(&workspace, None);
    let kinds: Vec<(&str, AgentKind)> = config
        .agents
        .iter()
        .map(|agent| (agent.name.as_str(), agent.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("build", AgentKind::Primary),
            ("plan", AgentKind::Primary),
            ("general", AgentKind::Subagent),
            ("explore", AgentKind::Subagent),
            ("orchestrator", AgentKind::Primary),
            ("title", AgentKind::Action),
            ("compaction", AgentKind::Action),
        ]
    );
    assert!(
        !config.agent("explore").unwrap().tools.contains(&"edit".to_string()),
        "explore is read-only"
    );
    let orchestrator = config.agent("orchestrator").unwrap();
    assert!(
        ["edit", "write", "apply_patch", "bash"]
            .iter()
            .all(|tool| !orchestrator.allows_tool(tool))
            && orchestrator.allows_tool("task"),
        "the orchestrator delegates; it never changes or runs anything itself"
    );
    assert!(orchestrator.prompt.contains("<orchestrator_status>"));
    assert!(config.agent("plan").unwrap().read_only && config.agent("explore").unwrap().read_only);
    assert!(config.commands.is_empty() && config.skills.is_empty() && config.instructions.is_empty());

    std::fs::remove_dir_all(workspace).ok();
}

#[test]
fn agent_tool_lists_in_every_shape_restrict_and_match_any_case() {
    let workspace = std::env::temp_dir().join(format!("drift-config-tools-{}", crate::random_hex(4)));
    write(
        &workspace,
        ".drift/agents/listed.md",
        "---\ndescription: Read-only\ntools:\n  - read\n  - grep\n---\nReview.",
    );
    write(
        &workspace,
        ".drift/agents/claude.md",
        "---\ndescription: Claude style\ntools: Read, Grep\n---\nReview.",
    );
    write(
        &workspace,
        ".drift/agents/opencode.md",
        "---\ndescription: No writes\ntools:\n  write: false\n  edit: false\n  bash: true\n---\nLook.",
    );
    write(
        &workspace,
        ".drift/agents/none.md",
        "---\ndescription: Talks only\ntools: []\n---\nTalk.",
    );
    write(
        &workspace,
        ".drift/agents/all.md",
        "---\ndescription: Everything\ntools: {}\n---\nDo.",
    );

    let config = Config::load_with_home(&workspace, None);
    for name in ["listed", "claude"] {
        let agent = config.agent(name).unwrap();
        assert!(agent.allows_tool("read") && agent.allows_tool("grep"), "{name}");
        assert!(
            !agent.allows_tool("edit") && !agent.allows_tool("bash"),
            "{name} is not handed every tool"
        );
    }
    let opencode = config.agent("opencode").unwrap();
    assert!(
        opencode.allows_tool("bash") && opencode.allows_tool("read"),
        "a true entry does not narrow the rest"
    );
    assert!(!opencode.allows_tool("write") && !opencode.allows_tool("edit"));
    assert!(
        !config.agent("none").unwrap().allows_tool("read"),
        "an empty list means no tools"
    );
    assert!(
        config.agent("all").unwrap().allows_tool("bash"),
        "an empty map means every tool"
    );
    assert!(
        config.agent("build").unwrap().allows_tool("anything"),
        "no list means every tool"
    );

    let mut widened = config.agent("explore").unwrap().clone();
    widened.tools = vec!["*".into()];
    assert!(
        widened.allows_tool("edit") && widened.allows_tool("bash"),
        "a Settings override can widen a narrowed agent back to every tool"
    );
    std::fs::remove_dir_all(workspace).ok();
}

#[test]
fn command_arguments_fill_placeholders_or_follow_the_template() {
    let command = |template: &str| Command::new("c".into(), String::new(), template.into());
    assert_eq!(
        command("Run tests for $ARGUMENTS.").expand(" src/a.rs  "),
        "Run tests for src/a.rs."
    );
    assert_eq!(
        command("Move $1 to $2").expand("a.rs lib/b c.rs"),
        "Move a.rs to lib/b c.rs",
        "the highest takes the rest"
    );
    assert_eq!(command("Only $1").expand(""), "Only ");
    assert_eq!(
        command("Review the diff.\n").expand("focus on errors"),
        "Review the diff.\n\nfocus on errors",
        "not dropped"
    );
    assert_eq!(command("Review the diff.").expand(""), "Review the diff.");
    assert_eq!(
        command("Commit as $1 with $2").expand(r#""Kyle P" 'fix the "parser" bug'"#),
        r#"Commit as Kyle P with fix the "parser" bug"#,
        "quotes keep spaces, as in a shell"
    );
    let ten = command("$1|$2|$3|$4|$5|$6|$7|$8|$9|$10|$11");
    assert_eq!(
        ten.expand("a b c d e f g h i j k l"),
        "a|b|c|d|e|f|g|h|i|j|k l",
        "past $9, and $1 never eats the start of $10"
    );
    assert_eq!(
        split_arguments(r#"one "two three"  '' four"#),
        ["one", "two three", "", "four"],
        "an empty quoted argument is still one"
    );
    assert_eq!(highest_placeholder("cost $5 and $12, not $ARGUMENTS"), 12);
}

#[test]
fn instructions_come_from_home_and_every_directory_up_to_the_repo_root() {
    let root = std::env::temp_dir().join(format!("drift-config-chain-{}", crate::random_hex(4)));
    let (home, repository) = (root.join("home"), root.join("repo"));
    let workspace = repository.join("apps/web");
    write(&home, ".config/drift/AGENTS.md", "mine everywhere");
    write(&home, ".claude/CLAUDE.md", "not read when Drift's own exists");
    write(&repository, ".git/HEAD", "ref: refs/heads/main");
    write(&repository, "AGENTS.md", "repo rules");
    write(&repository, "apps/CLAUDE.md", "apps rules");
    write(&workspace, "AGENTS.md", "web rules");
    write(&root, "AGENTS.md", "outside the repo, never read");

    let config = Config::load_with_home(&workspace, Some(&home));
    let names: Vec<&str> = config
        .instructions
        .iter()
        .map(|instruction| instruction.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "~/.config/drift/AGENTS.md",
            "../../AGENTS.md",
            "../CLAUDE.md",
            "AGENTS.md"
        ]
    );
    assert_eq!(config.instructions[0].text, "mine everywhere");
    std::fs::remove_file(home.join(".config/drift/AGENTS.md")).unwrap();
    let fallback = Config::load_with_home(&workspace, Some(&home));
    assert_eq!(fallback.instructions[0].name, "~/.claude/CLAUDE.md");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn listed_instructions_take_globs_absolute_paths_and_home() {
    let root = std::env::temp_dir().join(format!("drift-config-listed-{}", crate::random_hex(4)));
    let (home, workspace, elsewhere) = (root.join("home"), root.join("ws"), root.join("shared"));
    write(&home, "notes/style.md", "home style");
    write(&workspace, "docs/rules/a.md", "rule a");
    write(&workspace, "docs/rules/deep/b.md", "rule b");
    write(&workspace, "docs/rules/skip.txt", "not markdown");
    write(&elsewhere, "team.md", "team rules");
    let absolute = elsewhere.join("team.md").to_string_lossy().replace('\\', "/");
    write(
        &workspace,
        "drift.json",
        &format!(r#"{{ "instructions": ["docs/rules/**/*.md", "~/notes/style.md", "{absolute}", "missing.md"] }}"#),
    );

    let config = Config::load_with_home(&workspace, Some(&home));
    let listed: Vec<(&str, &str)> = config
        .instructions
        .iter()
        .filter(|instruction| !instruction.name.ends_with("AGENTS.md"))
        .map(|instruction| (instruction.name.as_str(), instruction.text.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("a.md", "rule a"),
            ("deep/b.md", "rule b"),
            ("~/notes/style.md", "home style"),
            (absolute.as_str(), "team rules")
        ]
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_jsonc_drift_json_applies_and_a_broken_one_is_reported_not_ignored() {
    let workspace = std::env::temp_dir().join(format!("drift-config-jsonc-{}", crate::random_hex(4)));
    write(
        &workspace,
        "drift.json",
        "{\n  // never push\n  \"permissions\": [{ \"kind\": \"bash\", \"pattern\": \"git push*\", \"decision\": \"deny\", },],\n}",
    );
    let config = Config::load_with_home(&workspace, None);
    assert!(config.problems.is_empty(), "{:?}", config.problems);
    assert_eq!(config.permissions.len(), 1);

    write(
        &workspace,
        "drift.json",
        r#"{ "permissions": [{ "kind": "bash" "pattern": "*" }] }"#,
    );
    let broken = Config::load_with_home(&workspace, None);
    assert!(broken.permissions.is_empty());
    assert!(
        broken.problems[0].contains("drift.json could not be read") && broken.problems[0].contains("permission rules"),
        "{:?}",
        broken.problems
    );

    std::fs::remove_dir_all(workspace).ok();
}
