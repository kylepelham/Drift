use super::*;

impl Engine {
    /// Holds named files before preparing their approval preview, until the call is recorded.
    pub(super) async fn lock_call(
        &self,
        scope: &CallScope<'_>,
        row: &mut PartRow,
        paths: Option<&[PathBuf]>,
    ) -> Result<Option<crate::tool::lock::Held>, Outcome> {
        let Some(paths) = paths else { return Ok(None) };
        if let Some(held) = self.wait_turn(&scope.plan.workspace, Some(paths), scope.abort).await {
            return Ok(Some(held));
        }

        self.settle(
            row,
            Settlement::error("Aborted while waiting for another write to these files.".into()),
        );
        Err(Outcome::Aborted)
    }

    /// Captures an approved writing call under its already-held file locks.
    pub(super) async fn before_write(
        &self,
        scope: &CallScope<'_>,
        row: &mut PartRow,
        touches: Option<Option<Vec<PathBuf>>>,
    ) -> Result<Option<changes::Capture>, Outcome> {
        let Some(touches) = touches else { return Ok(None) };
        self.snapshots
            .bind(&scope.plan.session.workspace_id, &scope.plan.workspace);

        // Whole-tree calls reuse the preceding tree; a named-file write clears that chain.
        let chained = scope.tree.lock().unwrap().take().filter(|_| touches.is_none());
        let captured = match chained {
            Some(tree) => Ok(changes::Capture::Tree(tree)),
            None => tokio::select! {
                captured = self.capture_before(&scope.plan.workspace, touches) => captured,
                () = scope.abort.cancelled() => {
                    self.settle(row, Settlement::error("Aborted while recording the files first.".into()));
                    return Err(Outcome::Aborted);
                }
            },
        };

        match captured {
            Ok(capture) => Ok(Some(capture)),
            Err(error) => {
                let reason = format!("refused to write: could not record the files first ({error})");
                self.settle(row, Settlement::error(reason));
                Err(Outcome::Allowed)
            }
        }
    }

    /// The turn to write `paths` in `workspace`, or all of it for `None`; `None` back if stopped while waiting.
    pub(super) async fn wait_turn(
        &self,
        workspace: &Path,
        paths: Option<&[PathBuf]>,
        abort: &CancellationToken,
    ) -> Option<crate::tool::lock::Held> {
        let turn = async {
            match paths {
                Some(paths) => crate::tool::lock::files(paths).await,
                None => crate::tool::lock::workspace(workspace).await,
            }
        };

        tokio::select! {
            held = turn => Some(held),
            () = abort.cancelled() => None,
        }
    }

    /// Reports a failed history capture in the result, failing the call when its files were put back.
    pub(super) async fn history_of(
        &self,
        scope: &CallScope<'_>,
        capture: changes::Capture,
        status: ToolStatus,
        text: String,
    ) -> (ToolStatus, String, Option<ToolMetadata>) {
        let plan = scope.plan;
        let owner = &plan.session.workspace_id;
        let recorded = self.record_call(&plan.workspace, capture).await;

        // The next whole-tree call begins at this call's after state, so it observes intervening changes.
        *scope.tree.lock().unwrap() = recorded.as_ref().ok().and_then(|recorded| recorded.tree.clone());
        match recorded {
            Ok(recorded) => {
                let mut changes = ToolMetadata {
                    changes: Some(recorded.changes.into_iter().map(Into::into).collect()),
                    owner: Some(owner.clone()),
                    at: Some(recorded.at),
                    ..Default::default()
                };
                if !recorded.unrecorded.is_empty() {
                    changes.unrecorded = Some(recorded.unrecorded);
                }

                (status, text, Some(changes))
            }
            Err(lost) => {
                let status = if lost.put_back { ToolStatus::Error } else { status };
                let history = ToolMetadata {
                    changes: Some(Vec::new()),
                    owner: Some(owner.clone()),
                    unrecorded: Some(lost.unrecorded),
                    history_error: Some(lost.note),
                    ..Default::default()
                };

                (status, text, Some(history))
            }
        }
    }

    /// Formats what a mutating call wrote; the model hears when a formatter changed it. Checks wait for the step's end.
    pub(super) async fn after_write(
        &self,
        scope: &CallScope<'_>,
        call_id: &str,
        mut text: String,
        metadata: ToolMetadata,
    ) -> (String, ToolMetadata) {
        let asker = trust::Asker {
            message_id: &scope.message.id,
            call_id,
            abort: scope.abort,
        };
        let files: Vec<PathBuf> = metadata.file_paths().map(PathBuf::from).collect();
        let config = &scope.plan.config;

        // Formatters and checks have separate approvals, so refusing one does not refuse the other.
        let written = files.clone();
        let workspace = scope.plan.workspace.clone();
        let formatters = crate::edit::format::resolve(&config.formatters);
        let programs = tokio::task::spawn_blocking(move || {
            crate::edit::format::project_programs(&written, &workspace, &formatters)
        })
        .await
        .unwrap_or_default();

        let lines: Vec<String> = config
            .project_command_lines("formatter", &files)
            .into_iter()
            .chain(programs.iter().map(|program| program.line.clone()))
            .collect();

        let allowed = self.project_commands_allowed(scope.plan, asker, lines).await;
        let overrides = config.only_allowed(|line| allowed.contains(line)).0;
        let local: Vec<PathBuf> = programs
            .into_iter()
            .filter(|program| allowed.contains(&program.line))
            .map(|program| program.path)
            .collect();

        let formatted = self.format_written(scope.plan, &metadata, &overrides, &local).await;
        let mut metadata = metadata;
        if !formatted.is_empty() {
            crate::tool::add_note(&mut text, &mut metadata, &reformatted_note(&formatted));
        }

        // Language servers read the files after formatting; Stop cancels their report wait.
        let found = tokio::select! {
            found = self.lsp.report(&scope.plan.workspace, &files, &config.lsp) => found,
            () = scope.abort.cancelled() => Vec::new(),
        };
        if let Some(note) = crate::lsp::note(&found, &scope.plan.workspace) {
            crate::tool::add_note(&mut text, &mut metadata, &note);
        }

        if !formatted.is_empty() {
            metadata.formatted = Some(formatted);
        }
        if !found.is_empty() {
            metadata.diagnostics = Some(crate::lsp::metadata(&found, &scope.plan.workspace));
        }

        (text, metadata)
    }

    /// Runs the workspace's formatters over whatever a mutating tool reported writing; names the files they changed.
    async fn format_written(
        &self,
        plan: &Plan,
        metadata: &ToolMetadata,
        overrides: &std::collections::BTreeMap<String, crate::config::FormatterConfig>,
        local: &[PathBuf],
    ) -> Vec<String> {
        let formatters = crate::edit::format::resolve(overrides);
        let mut formatted = Vec::new();
        for file in metadata.file_paths() {
            let before = tokio::fs::read(file).await.ok();
            let Some(name) =
                crate::edit::format::format(Path::new(file), &plan.workspace, &formatters, &self.store, local).await
            else {
                continue;
            };
            if tokio::fs::read(file).await.ok() != before {
                let display = crate::tool::display(Path::new(file), &plan.workspace);
                formatted.push(format!("{name}: {display}"));
            }
        }

        formatted
    }
}

/// What the model needs to hear after a formatter rewrote its change: the file is not what it wrote.
fn reformatted_note(formatted: &[String]) -> String {
    format!(
        "A formatter then changed the result ({}). The file no longer matches what you wrote; \
         read it again before editing those lines.",
        formatted.join(", ")
    )
}
