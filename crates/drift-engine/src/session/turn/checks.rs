use crate::edit::check::{Check, Report, Verdict};

use super::*;

/// The most time one step's checks may take together.
const CHECK_BUDGET: Duration = Duration::from_secs(90);

impl Engine {
    /// Runs workspace checks once over the step's written files and attaches their reports to its last writing call.
    pub(super) async fn check_step(&self, scope: &CallScope<'_>) {
        let StepWrites { mut files, last } = std::mem::take(&mut *scope.wrote.lock().unwrap());
        let Some(row) = last.filter(|_| !scope.plan.config.checks.is_empty() && !files.is_empty()) else {
            return;
        };
        files.sort();
        files.dedup();

        let asker = trust::Asker {
            message_id: &scope.message.id,
            call_id: call_id_of(&row),
            abort: scope.abort,
        };
        let lines = scope.plan.config.project_command_lines("check", &files);
        let allowed = self.project_commands_allowed(scope.plan, asker, lines).await;
        let configured = scope.plan.config.only_allowed(|line| allowed.contains(line)).1;
        let checks = crate::edit::check::resolve(&configured);
        if checks.is_empty() || scope.abort.is_cancelled() {
            return;
        }

        // A whole-workspace check must hold and capture the whole workspace, not just the written files.
        let whole = checks
            .iter()
            .any(|check| !check.command.iter().any(|part| part.contains("$FILE")));
        let named = (!whole).then(|| files.clone());
        let Some(_turn) = self
            .wait_turn(&scope.plan.workspace, named.as_deref(), scope.abort)
            .await
        else {
            return;
        };
        let capture = self.capture_before(&scope.plan.workspace, named).await;
        let bytes = match &capture {
            Ok(_) => Vec::new(),
            Err(_) => futures_util::future::join_all(files.iter().map(tokio::fs::read))
                .await
                .into_iter()
                .map(Result::ok)
                .collect(),
        };

        // A stopped fixer may already have written, so history is recorded even after Stop.
        let reports = crate::edit::check::run(&files, &scope.plan.workspace, &checks, CHECK_BUDGET, scope.abort).await;
        let recorded = match capture {
            Ok(capture) => self
                .record_call(&scope.plan.workspace, capture)
                .await
                .map(|recorded| attribute(recorded, &checks, &files, &scope.plan.workspace)),
            Err(error) => Err(self.unrecorded_rewrites(scope, &files, bytes, &error).await),
        };
        self.report_checks(scope, row, &reports, recorded);
    }

    /// Reports files rewritten by checks when their before state could not be captured.
    async fn unrecorded_rewrites(
        &self,
        scope: &CallScope<'_>,
        files: &[PathBuf],
        before: Vec<Option<Vec<u8>>>,
        error: &changes::CaptureError,
    ) -> changes::Lost {
        let mut unrecorded = Vec::new();
        for (file, previous) in files.iter().zip(before) {
            if tokio::fs::read(file).await.ok() != previous {
                unrecorded.push(crate::tool::display(file, &scope.plan.workspace));
            }
        }
        let note = (!unrecorded.is_empty()).then(|| {
            format!(
                "Drift could not record these files before the checks ran ({error}), \
             so undo cannot put back what the checks rewrote."
            )
        });

        changes::Lost {
            note: note.unwrap_or_default(),
            put_back: false,
            unrecorded,
        }
    }

    /// Adds check reports and rewrites to the last writing call's result and undo history.
    fn report_checks(
        &self,
        scope: &CallScope<'_>,
        mut row: PartRow,
        reports: &[Report],
        recorded: Result<changes::Recorded, changes::Lost>,
    ) {
        let workspace = &scope.plan.workspace;
        let (changes, unrecorded, lost) = match recorded {
            Ok(recorded) => (recorded.changes, Vec::new(), None),
            Err(lost) => (
                Vec::new(),
                lost.unrecorded,
                Some(lost.note).filter(|note| !note.is_empty()),
            ),
        };
        let changed: Vec<String> = changes
            .iter()
            .filter(|change| !change.observed)
            .map(|change| change.path.clone())
            .chain(unrecorded.iter().cloned())
            .collect();
        let elsewhere: Vec<String> = changes
            .iter()
            .filter(|change| change.observed)
            .map(|change| change.path.clone())
            .collect();
        let found = checks_note(reports, workspace, |label, said| {
            self.turns.repeated(&scope.plan.session.id, label, said)
        });
        let notes: Vec<String> = [found, changed_note(&changed), elsewhere_note(&elsewhere), lost]
            .into_iter()
            .flatten()
            .collect();

        let Part::ToolCall { output, metadata, .. } = &mut row.part else {
            return;
        };
        let mut text = output.take().unwrap_or_default();
        let mut meta = metadata.take().unwrap_or_default();
        for note in &notes {
            crate::tool::add_note(&mut text, &mut meta, note);
        }
        *output = Some(text);
        meta.checks = Some(checks_metadata(reports, workspace));
        if !changed.is_empty() {
            meta.check_changed = Some(changed);
        }
        if !elsewhere.is_empty() {
            meta.check_observed = Some(elsewhere);
        }
        if !unrecorded.is_empty() {
            meta.unrecorded
                .get_or_insert_default()
                .extend(unrecorded.iter().cloned());
        }
        if !changes.is_empty() {
            // Appending check rewrites after the call's changes makes undo restore check rewrites first.
            meta.changes
                .get_or_insert_default()
                .extend(changes.iter().cloned().map(Into::into));
        }
        *metadata = Some(meta);

        // Publish only saved results so the user and the next model request see the same history.
        if self.store.save_part(&row).is_ok() {
            self.hub.publish(Event::PartUpdated { part: row });
        }
    }
}

impl StepWrites {
    /// Adds what a settled writing call reports writing.
    pub(super) fn note(&mut self, row: &PartRow) {
        let Part::ToolCall {
            metadata: Some(metadata),
            ..
        } = &row.part
        else {
            return;
        };
        let files: Vec<PathBuf> = metadata.file_paths().map(Into::into).collect();
        if files.is_empty() {
            return;
        }

        self.files.extend(files);
        self.last = Some(row.clone());
    }
}

/// Only changes to written files covered by a check can be attributed to that check.
/// Other changes may belong to another writer, so undo leaves them alone.
fn attribute(
    mut recorded: changes::Recorded,
    checks: &[Check],
    written: &[PathBuf],
    workspace: &Path,
) -> changes::Recorded {
    let written: Vec<String> = written.iter().map(|file| changes::relative(workspace, file)).collect();
    for change in &mut recorded.changes {
        let covered = crate::edit::check::covers(checks, Path::new(&change.path));
        change.observed = !(written.contains(&change.path) && covered);
    }

    recorded
}

/// Names files changed outside this step's writes, whether by a workspace fixer or another writer.
/// The model must reread them, but undo does not restore them.
fn elsewhere_note(changed: &[String]) -> Option<String> {
    (!changed.is_empty()).then(|| {
        format!(
            "While the checks ran, files this step did not write changed too ({}), \
         by a whole-workspace check or by someone else. Read them again before relying on what you knew of them; \
         undo leaves them as they are.",
            changed.join(", ")
        )
    })
}

fn changed_note(changed: &[String]) -> Option<String> {
    (!changed.is_empty()).then(|| {
        format!(
            "A check then changed {}. Those files no longer match what you wrote; \
         read them again before editing those lines.",
            changed.join(", ")
        )
    })
}

fn check_label(report: &Report, workspace: &Path) -> String {
    match &report.file {
        Some(file) => format!("{}: {}", report.name, crate::tool::display(file, workspace)),
        None => report.name.clone(),
    }
}

/// Reports check problems, replacing output already reported with a short unchanged-problems notice.
fn checks_note(reports: &[Report], workspace: &Path, repeated: impl Fn(&str, Option<&str>) -> bool) -> Option<String> {
    let mut problems = Vec::new();
    for report in reports {
        let label = check_label(report, workspace);
        match &report.verdict {
            Verdict::Problems(said) if repeated(&label, Some(said)) => {
                problems.push(format!("[{label}] the same problems as reported before"));
            }
            Verdict::Problems(said) => problems.push(format!("[{label}]\n{said}")),
            Verdict::Passed => {
                repeated(&label, None);
            }
            Verdict::Unavailable(_) => {}
        }
    }

    (!problems.is_empty()).then(|| {
        format!(
            "Checks reported problems after this step's changes. Fix the ones your changes caused:\n\n{}",
            problems.join("\n\n")
        )
    })
}

fn checks_metadata(reports: &[Report], workspace: &Path) -> Vec<ToolCheck> {
    reports
        .iter()
        .map(|report| {
            let (status, output) = match &report.verdict {
                Verdict::Passed => (CheckStatus::Passed, None),
                Verdict::Problems(said) => (CheckStatus::Problems, Some(said.clone())),
                Verdict::Unavailable(why) => (CheckStatus::Unavailable, Some(why.clone())),
            };

            ToolCheck {
                check: check_label(report, workspace),
                status,
                output,
                extra: Default::default(),
            }
        })
        .collect()
}
