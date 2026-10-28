use super::*;
use drift_engine::config::{AgentKind, Config};
use drift_engine::permission::{Decision, Policy, Rule};
use drift_engine::session::compaction::AUTO_COMPACT_KEY;
use drift_engine::session::types::ModelRef;

struct Imported {
    fixture: Fixture,
    report: SettingsReport,
}

impl Imported {
    fn home(&self) -> PathBuf {
        self.fixture.dir.0.join("home")
    }

    /// What Drift itself reads from the written files, for a workspace with none of its own.
    fn drift(&self) -> Config {
        let workspace = self.fixture.dir.0.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        Config::load_with_home(&workspace, Some(&self.home()))
    }

    fn file(&self) -> drift_engine::config::File {
        serde_json::from_value(read_config(&self.home())).unwrap()
    }

    fn left_out(&self, name: &str) -> bool {
        self.report.left_out.settings.iter().any(|left| left == name)
    }
}

fn import(config: Value, prepare: impl FnOnce(&Fixture)) -> Imported {
    let fixture = Fixture::new();
    prepare(&fixture);
    let settings = settings(&fixture.dir.0, json!({}), config, vec![]);
    let report = fixture.import(&fixture.dir.0.join("home"), &settings);

    Imported { fixture, report }
}

fn decide(rules: &[Rule], kind: &str, target: &str) -> Option<Decision> {
    let policy = Policy {
        rules: rules.to_vec(),
        ..Default::default()
    };
    policy.explicit(&drift_engine::tool::Ask::new(kind, target, ""))
}

fn model(provider: &str, model: &str) -> Option<ModelRef> {
    Some(ModelRef {
        provider: provider.into(),
        model: model.into(),
    })
}

fn v2_config() -> Value {
    json!({
        "model": "anthropic/claude-opus-5-5#high",
        "permissions": [
            { "action": "shell", "resource": "*", "effect": "ask" },
            { "action": "shell", "resource": "git *", "effect": "allow" },
            { "action": "edit", "resource": "*.lock", "effect": "deny" },
            { "action": "subagent", "resource": "*", "effect": "deny" },
            { "action": "external_directory", "resource": "*", "effect": "ask" },
        ],
        "agents": { "reviewer": {
            "system": "Review carefully.", "description": "Reviews changes", "mode": "subagent",
            "model": "anthropic/claude-sonnet-4-5#high", "color": "#ff0000",
            "permissions": [
                { "action": "edit", "resource": "*", "effect": "deny" },
                { "action": "edit", "resource": "notes.md", "effect": "allow" },
            ],
        } },
        "commands": { "review": {
            "template": "Review $1", "description": "Review a file", "model": "openai/gpt-6#low", "subagent": true,
        } },
        "providers": { "acme": {
            "name": "Acme", "package": "aisdk:@ai-sdk/openai-compatible",
            "settings": { "baseURL": "https://llm.example.com/v1", "apiKey": "{env:ACME_KEY}" },
            "models": { "big": {
                "name": "Big", "limit": { "context": 200000, "output": 8000 },
                "capabilities": { "input": ["text", "image"] }, "cost": [],
            } },
        } },
        "snapshots": false,
        "compaction": { "auto": false, "keep": { "tokens": 8000 }, "buffer": 20000 },
    })
}

fn assert_v2_file(imported: &Imported) {
    let file = imported.file();
    assert_eq!(file.model, model("anthropic", "claude-opus-5-5"));
    assert_eq!(decide(&file.permissions, "bash", "git status"), Some(Decision::Allow));
    assert_eq!(decide(&file.permissions, "bash", "rm -rf x"), Some(Decision::Ask));
    assert_eq!(decide(&file.permissions, "edit", "Cargo.lock"), Some(Decision::Deny));
    assert_eq!(
        decide(&file.permissions, "task", "explore"),
        Some(Decision::Deny),
        "subagent is task"
    );

    let acme = &file.providers["acme"];
    assert_eq!(
        (
            acme.name.as_deref(),
            acme.base_url.as_deref(),
            acme.api_key_env.as_deref()
        ),
        (Some("Acme"), Some("https://llm.example.com/v1"), Some("ACME_KEY"))
    );
    let big = &acme.models["big"];
    assert_eq!(
        (big.name.as_deref(), big.context, big.output, big.images),
        (Some("Big"), 200_000, 8000, true)
    );
}

fn assert_v2_definitions(drift: &Config) {
    let reviewer = drift.agent("reviewer").unwrap();
    assert_eq!(
        (reviewer.prompt.as_str(), reviewer.description.as_str(), reviewer.kind),
        ("Review carefully.", "Reviews changes", AgentKind::Subagent)
    );
    assert_eq!(
        (&reviewer.model, reviewer.variant.as_deref()),
        (&model("anthropic", "claude-sonnet-4-5"), Some("high"))
    );
    let policy = drift.agent_policy("reviewer");
    assert_eq!(
        decide(&policy.rules, "edit", "notes.md"),
        Some(Decision::Allow),
        "the later rule wins"
    );
    assert_eq!(decide(&policy.rules, "edit", "a.rs"), Some(Decision::Deny));

    let review = drift.commands.iter().find(|command| command.name == "review").unwrap();
    assert_eq!(
        (
            review.template.as_str(),
            review.description.as_str(),
            &review.model,
            review.subtask
        ),
        ("Review $1", "Review a file", &model("openai", "gpt-6"), Some(true))
    );
}

#[test]
fn a_v2_config_becomes_drift_json_agents_and_commands() {
    let imported = import(v2_config(), |_| {});

    assert_v2_file(&imported);
    assert_v2_definitions(&imported.drift());
    assert_eq!(imported.report.files, ["agents/reviewer.md", "commands/review.md"]);
    for name in [
        "model.variant",
        "permissions.external_directory",
        "agents.reviewer.color",
        "commands.review.model.variant",
        "providers.acme.package",
        "providers.acme.models.big.cost",
        "snapshots",
        "compaction.keep.tokens",
        "compaction.buffer",
    ] {
        assert!(
            imported.left_out(name),
            "{name}: {:?}",
            imported.report.left_out.settings
        );
    }
    assert_eq!(
        imported.fixture.engine.store.setting::<bool>(AUTO_COMPACT_KEY).unwrap(),
        Some(false),
        "compaction.auto is Drift's own switch"
    );
}

fn v1_config() -> Value {
    json!({
        "agent": { "docs": {
            "prompt": "{file:prompts/docs.txt}", "model": "anthropic/claude-sonnet-4-5", "variant": "max",
            "tools": { "write": false, "read": true }, "temperature": 0.2,
            "permission": { "bash": { "*": "ask", "git *": "allow" }, "doom_loop": "ask" },
        } },
        "mode": { "focus": { "prompt": "Stay on task." } },
        "command": { "fix": { "template": "Fix $ARGUMENTS", "variant": "high", "subtask": true, "agent": "build" } },
        "provider": { "local": {
            "api": "http://box:8080/v1", "env": ["LOCAL_KEY"], "npm": "@ai-sdk/openai-compatible",
            "options": { "apiKey": "sk-written-out", "timeout": 5000 },
        } },
        "snapshot": false,
        "compaction": { "auto": true, "preserve_recent_tokens": 8000, "tail_turns": 2 },
        "plugin": ["opencode-a", ["./plugin/b.ts", { "on": true }]],
    })
}

fn assert_v1_definitions(drift: &Config) {
    let docs = drift.agent("docs").unwrap();
    assert_eq!(
        (docs.prompt.as_str(), docs.variant.as_deref(), &docs.tools),
        ("Write the docs.", Some("max"), &vec!["!write".to_string()]),
        "the prompt file is read as opencode reads it"
    );
    let policy = drift.agent_policy("docs");
    assert_eq!(decide(&policy.rules, "bash", "git log"), Some(Decision::Allow));
    assert_eq!(decide(&policy.rules, "bash", "rm x"), Some(Decision::Ask));
    assert_eq!(
        drift.agent("focus").unwrap().kind,
        AgentKind::Primary,
        "a mode is a primary agent"
    );

    let fix = drift.commands.iter().find(|command| command.name == "fix").unwrap();
    assert_eq!(
        (fix.template.as_str(), fix.agent.as_deref(), fix.subtask),
        ("Fix $ARGUMENTS", Some("build"), Some(true))
    );
}

#[test]
fn a_v1_config_brings_its_agents_commands_and_providers_too() {
    let imported = import(v1_config(), |fixture| {
        std::fs::create_dir_all(fixture.dir.0.join("prompts")).unwrap();
        std::fs::write(fixture.dir.0.join("prompts/docs.txt"), "Write the docs.\n").unwrap();
    });

    assert_v1_definitions(&imported.drift());
    let local = &imported.file().providers["local"];
    assert_eq!(
        (local.base_url.as_deref(), local.api_key_env.as_deref()),
        (Some("http://box:8080/v1"), Some("LOCAL_KEY"))
    );
    for name in [
        "agent.docs.temperature",
        "agent.docs.permission.doom_loop",
        "command.fix.variant",
        "provider.local.npm",
        "provider.local.options.apiKey",
        "provider.local.options.timeout",
        "snapshot",
        "compaction.keep.tokens",
        "compaction.tail_turns",
    ] {
        assert!(
            imported.left_out(name),
            "{name}: {:?}",
            imported.report.left_out.settings
        );
    }
    assert_eq!(imported.report.left_out.plugins, ["opencode-a", "./plugin/b.ts"]);
    assert_eq!(
        imported.fixture.engine.store.setting::<bool>(AUTO_COMPACT_KEY).unwrap(),
        Some(true)
    );
}

fn mixed_config() -> Value {
    json!({
        "permission": { "bash": "deny", "edit": "allow" },
        "permissions": [{ "action": "shell", "resource": "ls *", "effect": "allow" }],
        "agent": { "reviewer": { "prompt": "v1 prompt" }, "mine": { "prompt": "opencode's" } },
        "agents": { "reviewer": { "system": "v2 prompt" } },
        "provider": { "acme": { "api": "http://old/v1" } },
        "providers": { "acme": { "settings": { "baseURL": "http://new/v1" } } },
        "compaction": { "auto": false },
    })
}

#[test]
fn v1_and_v2_keys_side_by_side_come_in_with_v2_winning_and_drifts_own_kept() {
    let imported = import(mixed_config(), |fixture| {
        let agents = fixture.dir.0.join("home/.config/drift/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("mine.md"), "My own.").unwrap();
        fixture.engine.store.set_setting(AUTO_COMPACT_KEY, &true).unwrap();
    });

    let file = imported.file();
    assert_eq!(
        decide(&file.permissions, "bash", "ls -la"),
        Some(Decision::Allow),
        "v2's rule comes after v1's"
    );
    assert_eq!(decide(&file.permissions, "bash", "rm x"), Some(Decision::Deny));
    assert_eq!(decide(&file.permissions, "edit", "a.rs"), Some(Decision::Allow));
    assert_eq!(file.providers["acme"].base_url.as_deref(), Some("http://new/v1"));

    let drift = imported.drift();
    assert_eq!(drift.agent("reviewer").unwrap().prompt, "v2 prompt");
    assert_eq!(
        drift.agent("mine").unwrap().prompt,
        "My own.",
        "a definition already there is kept"
    );
    assert_eq!(
        imported.fixture.engine.store.setting::<bool>(AUTO_COMPACT_KEY).unwrap(),
        Some(true),
        "Drift's own switch is kept"
    );
}

#[test]
fn mcp_servers_come_from_both_shapes_and_v2s_disabled_stays_off() {
    let config = OcConfig::parse(
        &json!({ "mcp": {
            "docs": { "type": "remote", "url": "https://old.example/mcp" },
            "servers": {
                "docs": {
                    "type": "remote", "url": "https://docs.example/mcp",
                    "oauth": { "client_id": "app", "client_secret": "s" },
                },
                "off": { "type": "local", "command": ["x"], "disabled": true },
            },
            "timeout": { "catalog": 30000 },
        } })
        .to_string(),
    )
    .unwrap();
    let found = config.servers();
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["docs", "off"], "v2's wins for a name in both");

    let fixture = Fixture::new();
    let servers = found
        .into_iter()
        .map(|(name, definition)| OcServer {
            name,
            definition,
            approved: true,
        })
        .collect();
    let report = fixture.import(&fixture.dir.0, &settings(&fixture.dir.0, json!({}), json!({}), servers));

    assert_eq!(report.disabled_servers, ["off"]);
    let saved = fixture.engine.store.mcp_server("docs").unwrap().unwrap().config;
    assert!(
        matches!(&saved, drift_engine::mcp::ServerConfig::Http { url, oauth: Some(oauth), .. }
            if url == "https://docs.example/mcp" && oauth.client_id == "app" && oauth.client_secret.as_deref() == Some("s")),
        "{saved:?}"
    );
}
