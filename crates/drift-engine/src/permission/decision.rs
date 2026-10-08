use super::policy::fallback;
use super::{Compiled, Decision, Grant, Permissions, Policies, Policy};
use crate::tool::Ask;

struct CommandDecision<'a> {
    written: &'a str,
    canonical: Option<&'a String>,
    default: Decision,
}

struct TargetDecision<'a> {
    kind: &'a str,
    targets: &'a [&'a str],
    wildcards: bool,
    default: Decision,
}

impl Permissions {
    /// A shell line is judged command by command: any denied command denies it, and only a line whose
    /// every command is allowed runs without asking. A line that hides what it runs can only be
    /// allowed by an exact approval of the whole line; a wildcard rule never covers it. Reading a file
    /// that may hold secrets is held to the same bar, so `read *` never quietly covers `.env`.
    pub(super) fn decide(&self, session_id: &str, workspace: &Policy, ask: &Ask) -> Decision {
        let decision = self.decide_by_rules(session_id, workspace, ask);
        if decision == Decision::Ask && self.auto_accepts(session_id) {
            return Decision::Allow;
        }

        decision
    }

    fn decide_by_rules(&self, session_id: &str, workspace: &Policy, ask: &Ask) -> Decision {
        if ask.kind == "read" && crate::tool::sensitive::is_sensitive(std::path::Path::new(&ask.pattern)) {
            return self.decide_target(
                session_id,
                workspace,
                TargetDecision {
                    kind: "read",
                    targets: &ask.targets(),
                    wildcards: false,
                    default: fallback(ask.default_allow),
                },
            );
        }
        if ask.kind != "bash" {
            return self.decide_target(
                session_id,
                workspace,
                TargetDecision {
                    kind: &ask.kind,
                    targets: &ask.targets(),
                    wildcards: true,
                    default: fallback(ask.default_allow),
                },
            );
        }
        let Some(commands) = &ask.commands else {
            return self.decide_target(
                session_id,
                workspace,
                TargetDecision {
                    kind: "bash",
                    targets: &[&ask.pattern],
                    wildcards: false,
                    default: Decision::Ask,
                },
            );
        };

        let default = fallback(ask.default_allow);
        let decisions: Vec<Decision> = commands
            .iter()
            .enumerate()
            .map(|(index, command)| {
                self.decide_command(
                    session_id,
                    workspace,
                    CommandDecision {
                        written: command,
                        canonical: ask.canonical.get(index),
                        default,
                    },
                )
            })
            .collect();

        if decisions.contains(&Decision::Deny) {
            Decision::Deny
        } else if !ask.writes.is_empty() {
            // File-writing redirections require approval of the whole line, not just its program.
            self.decide_target(
                session_id,
                workspace,
                TargetDecision {
                    kind: "bash",
                    targets: &[&ask.pattern],
                    wildcards: false,
                    default,
                },
            )
        } else if decisions.iter().all(|decision| *decision == Decision::Allow) {
            Decision::Allow
        } else {
            Decision::Ask
        }
    }

    /// One command as written; and, for deny rules only, as it actually runs (`FOO=1 git push` is a
    /// `git push`, PowerShell's `rm` is `Remove-Item`). Approvals and allow rules see only what was written.
    fn decide_command(&self, session_id: &str, workspace: &Policy, decision: CommandDecision<'_>) -> Decision {
        let CommandDecision {
            written: command,
            canonical,
            default,
        } = decision;
        let written = self.decide_target(
            session_id,
            workspace,
            TargetDecision {
                kind: "bash",
                targets: &[command],
                wildcards: true,
                default,
            },
        );
        let Some(canonical) = canonical.filter(|canonical| !canonical.is_empty() && canonical.as_str() != command)
        else {
            return written;
        };

        let global = self.policy.lock().unwrap().rules.clone();
        let denied = workspace
            .rules
            .iter()
            .chain(global.iter())
            .find(|rule| rule.matches_target("bash", canonical))
            .is_some_and(|rule| rule.decision == Decision::Deny);

        if denied { Decision::Deny } else { written }
    }

    /// A deny rule first, so an "always" kept for the workspace never outlasts a rule added after it;
    /// then "always" answers (a subagent's parents' included); then the workspace's drift.json and the global policy.
    fn decide_target(&self, session_id: &str, workspace: &Policy, decision: TargetDecision<'_>) -> Decision {
        let TargetDecision {
            kind,
            targets,
            wildcards,
            default,
        } = decision;
        let global = self.policy.lock().unwrap().rules.clone();
        let rule = workspace
            .rules
            .iter()
            .chain(global.iter())
            .find(|rule| targets.iter().any(|target| rule.matches_target(kind, target)))
            .cloned();
        if rule.as_ref().is_some_and(|rule| rule.decision == Decision::Deny) {
            return Decision::Deny;
        }

        let lineage = self.lineage(session_id);
        let covers = |grant: &Grant| {
            (wildcards || matches!(grant, Grant::Exact { .. }))
                && targets.iter().any(|target| grant.allows(kind, target))
        };
        let by_session = lineage
            .iter()
            .filter_map(|id| {
                self.session_rules
                    .lock()
                    .unwrap()
                    .get(id)
                    .map(|grants| grants.iter().any(covers))
            })
            .any(|found| found);
        let workspaces: Vec<String> = lineage.iter().filter_map(|id| self.workspace_of(id)).collect();
        let by_workspace = workspaces.iter().any(|workspace| {
            self.workspace_rules
                .lock()
                .unwrap()
                .get(workspace)
                .is_some_and(|grants| grants.iter().any(covers))
        });
        if by_session || by_workspace {
            return Decision::Allow;
        }

        match rule {
            Some(rule) if rule.decision == Decision::Allow && !wildcards && rule.has_wildcards() => Decision::Ask,
            Some(rule) => rule.decision,
            None => default,
        }
    }

    /// What the rules and the session's approvals say right now, without asking anyone.
    pub fn decide_now(&self, session_id: &str, workspace: &Policy, ask: &Ask) -> Decision {
        self.decide(session_id, workspace, ask)
    }

    pub fn decide_under(&self, session_id: &str, workspace: &Policy, agent: &Policy, ask: &Ask) -> Decision {
        if agent.explicit(ask) == Some(Decision::Deny) {
            return Decision::Deny;
        }

        let rules = agent.rules.iter().chain(&workspace.rules).cloned().collect();
        self.decide(session_id, &Policy { rules }, ask)
    }

    /// The agent's, workspace's and global rules in that order, compiled once for checking many files.
    pub fn compiled(&self, workspace: &Policy, agent: &Policy) -> Compiled {
        let global = self.policy.lock().unwrap().rules.clone();

        Compiled::new(agent.rules.iter().chain(&workspace.rules).cloned().chain(global))
    }

    /// A file inside a search already approved: only an explicit rule can exclude it, and an ask rule yields to a session grant.
    pub fn covered_by_approval(&self, session_id: &str, rules: &Compiled, policies: Policies<'_>, ask: &Ask) -> bool {
        match rules.explicit(ask) {
            None | Some(Decision::Allow) => true,
            Some(Decision::Deny) => false,
            Some(Decision::Ask) => {
                self.decide_under(session_id, policies.workspace, policies.agent, ask) == Decision::Allow
            }
        }
    }
}
