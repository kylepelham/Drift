# Code review

`/review` asks the model to review the workspace's changes and report what it finds as a list the user can work
through: a card in the transcript, notes on the lines in the review panel, and what became of each finding once
fixes land. The design follows Claude Code's `/code-review`; the evidence is in
[research/claude-desktop-2.42304.md](research/claude-desktop-2.42304.md).

## The command

```
/review [low|medium|high|max] [target] [--fix] [--comment]
```

`review` is a skill the engine ships (`config::Builtin::Review`), added after every other skill folder and bundle,
so a skill or command of the user's, the project's or a bundle's named `review` replaces it. Like any skill it is a
slash command, routed through the `skill` tool so the transcript shows `/review high` and a skill row rather than the
prompt, and the model can load it by itself when asked for a review. Its stored instructions are only the usage
table the slash menu reads its levels from; the prompt is rendered per call by `crates/drift-engine/src/code_review`,
from the Markdown pieces in its `prompts` folder.

What it reviews:

- With no target, the current branch's work and everything not yet committed: the working tree against the
  merge-base with the first of `main`, `master`, `origin/main`, `origin/master`, `develop`, `origin/develop` that
  is not the current branch, else the upstream, else `HEAD`, plus untracked files. This is the same base the review
  panel's All changes picks. The engine measures it with `git diff --numstat` and counts untracked lines before
  writing the prompt; a clean tree or a folder outside git gets a prompt that says so and stops.
- A target replaces that: a pull request number or URL (`gh pr diff`), a branch, tag or commit, or a path.
- `REVIEW.md` at the repository root, when there is one, says what to review and what to skip and overrides the
  defaults. `AGENTS.md` and `CLAUDE.md` rules are already in the system prompt; a conventions finding quotes the
  rule and the line.

## Levels

| Level | Finders | Verify | Findings at most |
|---|---|---|---|
| low | none: one pass by the agent itself, test-only hunks skipped | none | 5 |
| medium (default) | 2 to 4 `explore` subagents | batched; plausible kept only for data loss, corruption or security | 8 |
| high | 3 to 6 | batched; confirmed and plausible kept | 10 |
| max (`xhigh` too) | 4 to 8, three more angles | batched, then a sweep for what the list missed | 15 |

Finders get about one per 150 changed lines within the level's range. The angles are line by line, removed
behaviour, callers and callees, reuse, simplification, efficiency, altitude (a fix at the wrong depth) and
conventions; max adds language pitfalls, wrappers and security. Claude Code runs one verifier per candidate; Drift
verifies in batches of about five, which is where most of the pipeline's cost went. Without the `task` tool the
agent works through the angles itself.

The main agent edits nothing during a review. `--fix` applies the findings after the report, skipping any whose
fix would undo intended behaviour, then reports outcomes. `--comment` posts each finding as an inline comment on
the pull request through `gh api` (lines outside its diff go in one `gh pr comment`), and never approves or
requests changes; without a pull request or `gh` it says the flag was ignored.

## Codebase review

```
/audit [low|medium|high|max] [path] [--fix]
```

`audit` is the second skill the engine ships (`config::Builtin::Audit`), added and replaceable the same way as
`review`. It reviews the code as it is now rather than a change: the whole workspace, or the folder or file `path`
names. Its prompt is rendered by `crates/drift-engine/src/code_review/audit` from its own `prompts` folder and the
pieces it shares with `/review` (conventions, verify, sweep, report, fix and afterwards). `--comment` does not
apply and is ignored.

The engine surveys the files before writing the prompt:

- `git ls-files --cached --others --exclude-standard`, limited to `path` when one is given. Lockfiles (`Cargo.lock`,
  `bun.lock`, `bun.lockb`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`), `.md`, `.mdx`, `.txt`, `.svg`,
  `.min.js` and `.map` files are left out, and so are files over 1 MB, binary files (a NUL byte) and empty ones.
  At most 20,000 files are counted, in one blocking task; the prompt says how many more were not.
- Areas: from the files' common folder down, a folder with more than about 4,000 lines splits into its children, at
  most 8 deep. The files sitting directly in a split folder form their own area, "files directly in X".
- Hot spots: the 12 files changed in most commits of the last 90 days (`git log --since=90.days.ago --name-only`,
  the newest 1,000 commits at most, so a busy repository stays a bounded read), and the 8 largest.
- Outside a git repository the prompt says so and asks the model to list the files with `glob`, skipping
  dependencies and build output, and split them by folder itself. A path with nothing to count gets a prompt that
  says so and stops.

| Level | Finders | Lines per finder | Findings at most |
|---|---|---|---|
| low | none: one pass by the agent, hot spots and entry points first, then each top-level folder | n/a | 8 |
| medium (default) | 3 to 6 | about 15,000 | 15 |
| high | 5 to 10 | about 15,000 | 25 |
| max | 8 to 16, three more angles, then the sweep | about 15,000 | 32 |

There are never more finders than areas. The engine balances the areas across finders by lines, largest first,
each to the finder with the fewest lines so far, and lists each finder's areas with their line counts. A codebase
is not read line by line: finders read the entry points and hot spots of their part in full and skim the rest. The
angles are correctness, resources and concurrency, contracts and wrappers, security, dead and duplicated code,
complexity, efficiency and conventions; max adds architecture, language pitfalls and tests. Verify and the sweep
are the ones `/review` runs.

The report passes `scope` `codebase` to `report_findings`, which keeps it in the call's metadata. The card then
reads "Codebase review", Re-run runs `/audit` at the same level, and Walk through opens the review panel on the
`files` source: `GET /workspaces/{id}/review?source=files&target=<paths>`, one workspace-relative path a line, the
findings' own files once each. Each file comes back as it is on disk with every line unchanged (status
`unchanged`, no additions or deletions, so the panel shows no counts), large, binary and unreadable files keep their
previews, missing files are left out, and an absolute path or one with `..` is refused. This source needs no
repository.

## `report_findings`

The model reports through the `report_findings` tool, once per review, instead of listing findings in its reply.
Its input is `level`, `scope` (`changes`, or `codebase` for an `/audit` review) and up to 32 findings, each with
`file`, `line`, `summary`, `short_summary` (60 characters), `failure_scenario`, `category`, `verdict` (`confirmed`
or `plausible`, when a verify pass ran) and `outcome` (`fixed`, `skipped` or `no_change_needed`, only when
re-reporting). It asks no permission and changes nothing.

The findings live in the call's metadata (`ToolMetadata.findings`, `level`, `scope`), so they stay in the session
record with no table of their own. The engine makes each `file` relative to the workspace with `/` separators and
records the text of each finding's line (`code`, files up to 1 MB, 400 characters), so the panel can tell a line
that moved from one that changed. When findings are fixed later in the conversation the model calls the tool again
with the same findings and their outcomes; the review prompt and the tool's description both ask for this, and
Apply fixes says so in its prompt.

## In the transcript

A report gets a card (`src/ui/findings-card.tsx`) in place of a tool row: "Code review · high · 5 findings", the
findings grouped by file with their line, a Plausible tag where the verify pass was not sure, and each finding's
outcome. A finding's outcome is its own, else the one the latest later report gives it, matched by file and
summary, then by file and line (`findingOutcomes` in `src/state/review-findings.ts`). A report that only records
outcomes stays a plain row; its card is the earlier one.

- Walk through in diff, or a finding's row, opens the review panel at that finding.
- Apply fixes sends the open findings (no outcome, not dismissed) as a prompt with the session's agent and model,
  queued behind a running turn.
- Re-run review runs `/review` again at the same level; an `/audit` card runs `/audit` at its level on the same
  path, which `report_findings` records as `path`.

## In the review panel

The panel opened from a card carries `findings: { sessionId, partId, current }`, and keeps it when the source
changes. Each finding is placed on its file's lines (`anchorFinding`): on its line number when the text there still
matches what was recorded, else on the nearest line with that text (marked as moved), else on its line number
(marked as changed, unless the finding is already fixed). A line with a finding is pinned, so it and its context never fold. The finding shows under its
line like a line note, with its category, verdict, outcome, summary and failure scenario.

A stepper above the files reads "Finding N of M" with the current finding's place and summary, Previous and Next,
Apply fixes, Dismiss and a close button. Moving to a finding opens and builds its file and the chunk of lines
holding it, and its note scrolls itself into view once. A finding whose file is not in the comparison says "Not in
this diff". Dismissing hides a finding from the panel and from Apply fixes until the app restarts; it is not stored.
