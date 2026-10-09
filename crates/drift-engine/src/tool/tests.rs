use super::*;
use crate::llm::catalog::ToolProfile;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) struct Sandbox {
    pub ctx: Context,
}

impl Sandbox {
    pub(crate) fn new(name: &str) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let root = std::env::temp_dir().join(format!("drift-tool-{name}-{}", crate::random_hex(4)));
        let workspace = root.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace = canonical(&workspace);
        let engine = crate::Engine::open_with(
            &root.join("data"),
            crate::Options {
                file_credentials: true,
                ..Default::default()
            },
        )
        .unwrap();

        Self {
            ctx: Context {
                agent: "build".into(),
                config: Arc::new(engine.workspace_config(&workspace)),
                progress: Default::default(),
                command_model: None,
                workspace,
                session_id: "ses_test".into(),
                message_id: "msg_test".into(),
                call_id: "call_test".into(),
                files: Arc::new(SessionFiles::default()),
                abort: CancellationToken::new(),
                engine,
            },
        }
    }

    pub(crate) fn ctx_clone(&self) -> Context {
        Context {
            agent: self.ctx.agent.clone(),
            workspace: self.ctx.workspace.clone(),
            session_id: self.ctx.session_id.clone(),
            message_id: self.ctx.message_id.clone(),
            call_id: self.ctx.call_id.clone(),
            files: self.ctx.files.clone(),
            abort: self.ctx.abort.clone(),
            engine: self.ctx.engine.clone(),
            config: self.ctx.config.clone(),
            progress: self.ctx.progress.clone(),
            command_model: self.ctx.command_model.clone(),
        }
    }

    pub(crate) fn file(&self, path: &str, content: &str) -> PathBuf {
        let full = self.ctx.workspace.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content).unwrap();

        full
    }

    /// The configuration as a turn admitted now would see it.
    pub(crate) fn reload_config(&mut self) {
        self.ctx.config = Arc::new(self.ctx.engine.workspace_config(&self.ctx.workspace));
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.ctx.workspace.parent().unwrap());
    }
}

#[test]
fn registry_exposes_every_builtin_with_a_schema() {
    let registry = Registry::builtin();
    let names: Vec<String> = registry
        .specs(ToolProfile::Edit)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        names,
        [
            "read",
            "write",
            "edit",
            "bash",
            "glob",
            "grep",
            "webfetch",
            "todowrite",
            "question",
            "skill",
            "task",
            "task_output",
            "task_stop",
            "read_thread"
        ]
    );
    let patching: Vec<String> = registry
        .specs(ToolProfile::ApplyPatch)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        patching,
        [
            "read",
            "apply_patch",
            "bash",
            "glob",
            "grep",
            "webfetch",
            "todowrite",
            "question",
            "skill",
            "task",
            "task_output",
            "task_stop",
            "read_thread"
        ]
    );

    for spec in registry
        .specs(ToolProfile::Edit)
        .into_iter()
        .chain(registry.specs(ToolProfile::ApplyPatch))
    {
        assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
        assert!(!spec.description.is_empty(), "{}", spec.name);
    }
    assert!(registry.get("read").is_some());
    assert!(registry.get("nope").is_none());
}

#[test]
fn the_scratch_directory_is_free_to_read_and_write_and_the_rest_of_temp_is_not() {
    let sandbox = Sandbox::new("scratch");
    let scratch = scratch_dir().join(format!("notes-{}.txt", crate::random_hex(4)));
    let elsewhere = canonical(&std::env::temp_dir().join("not-drift").join("notes.txt"));

    assert!(
        sandbox.ctx.ask_to_write(&scratch, "Write").unwrap().default_allow
            && sandbox.ctx.ask_to_read(&scratch, "Read").unwrap().default_allow
    );
    assert!(
        sandbox.ctx.ask_to_write(&elsewhere, "Write").is_some()
            && sandbox.ctx.ask_to_read(&elsewhere, "Read").is_some()
    );
    assert!(
        sandbox.ctx.ask_to_write(&scratch_dir(), "Write").is_some(),
        "the directory itself is not a file to write"
    );
    assert!(
        sandbox.ctx.ask_to_read(&scratch_dir().join(".env"), "Read").is_some(),
        "a secret is a secret even there"
    );
}

#[test]
fn workspace_edits_run_by_default_except_drifts_own_config_vcs_internals_and_secrets() {
    let sandbox = Sandbox::new("edit-defaults");
    let workspace = &sandbox.ctx.workspace;
    let allowed = |relative: &str| {
        sandbox
            .ctx
            .ask_to_write(&workspace.join(relative), "Edit")
            .unwrap()
            .default_allow
    };

    assert!(allowed("src/main.rs") && allowed("README.md"));
    for guarded in [
        "drift.json",
        "sub/drift.json",
        ".drift/agents/build.md",
        ".git/config",
        ".env",
        "secrets/id_rsa",
    ] {
        assert!(!allowed(guarded), "{guarded}");
    }
    let outside = canonical(&workspace.parent().unwrap().join("elsewhere.txt"));
    assert!(!sandbox.ctx.ask_to_write(&outside, "Edit").unwrap().default_allow);
}

#[test]
fn an_offered_skills_files_read_without_asking_and_its_secrets_still_ask() {
    let mut sandbox = Sandbox::new("skill-read");
    let skill_dir = canonical(
        &sandbox
            .ctx
            .workspace
            .parent()
            .unwrap()
            .join("home-skills")
            .join("review"),
    );
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    let mut config = (*sandbox.ctx.config).clone();
    config.skills.push(crate::config::Skill {
        name: "review".into(),
        description: "d".into(),
        path: skill_dir.to_string_lossy().into_owned(),
        instructions: String::new(),
        argument_hint: None,
    });
    sandbox.ctx.config = Arc::new(config);

    let read = |path: &Path| sandbox.ctx.ask_to_read(path, "Read").unwrap().default_allow;
    assert!(
        read(&skill_dir.join("references/guide.md")),
        "a file the skill points at"
    );
    assert!(!read(&skill_dir.join(".env")), "secrets still ask");
    assert!(
        !read(&skill_dir.parent().unwrap().join("other/SKILL.md")),
        "another, unoffered folder asks"
    );
    assert!(
        sandbox
            .ctx
            .ask_if_outside("read", &skill_dir, "Search")
            .unwrap()
            .default_allow,
        "and glob may search it"
    );
    assert!(
        !sandbox
            .ctx
            .ask_if_outside("edit", &skill_dir, "Edit")
            .unwrap()
            .default_allow,
        "reading only"
    );
}

#[test]
fn descriptions_promise_only_what_the_tools_do() {
    let megabytes = |bytes: usize| format!("{} MB", bytes / 1024 / 1024);
    let kilobytes = |bytes: usize| format!("{} KB", bytes / 1024);

    for text in [include_str!("prompts/edit.txt"), include_str!("prompts/write.txt")] {
        assert!(
            !text.contains("unified diff") && text.contains("not the diff"),
            "{text}"
        );
    }
    let fetch = include_str!("prompts/webfetch.txt");
    assert!(
        fetch.contains(&kilobytes(spool::MAX_RESULT_BYTES))
            && fetch.contains(&kilobytes(spool::HEAD_BYTES))
            && fetch.contains(&megabytes(webfetch::MAX_BYTES)),
        "{fetch}"
    );
    assert_eq!(
        spool::HEAD_BYTES,
        spool::TAIL_BYTES,
        "the text says first and last of one size"
    );

    let read = include_str!("prompts/read.txt");
    for promise in [
        megabytes(image::MAX_SOURCE_BYTES),
        format!("{} pixels", image::MAX_SIDE),
        megabytes(image::MAX_IMAGE_BYTES),
        megabytes(image::MAX_PDF_BYTES),
    ] {
        assert!(read.contains(&promise), "read.txt should say {promise}");
    }
    let task = include_str!("prompts/task.txt");
    assert!(
        task::DELEGATION.iter().all(|tool| task.contains(&format!("`{tool}`"))) && !task.contains("the same tools"),
        "{task}"
    );
}
