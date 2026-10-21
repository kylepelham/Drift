# Claude Desktop 2.42304: code review and other gaps

Investigated 2026-10-20 against the installed Windows package `Claude_2.42304.2.0_x64__pzs8sxrjxfjjc` and the
Claude Code build it runs, 2.1.299, from the package's own `claude-code` folder (not the 2.1.85 CLI the earlier
reports used). Drift was at `3226d6e9b` on `next/2.4.0`.

The question was what Drift lacks, and above all whether Claude's code review is worth adopting. It is: the
review is a pipeline of a prompt, a typed tool and UI that tracks each finding until it is fixed, and Drift
already has the diff panel and line notes the last part sits on. What Drift adopts is in `CHECKLIST.md`; this
report is evidence, not the plan.

## Method

`app.asar` was copied out of the package and extracted with `@electron/asar`. Its `ion-dist` folder holds the
bundled claude.ai web UI the desktop's Code tab runs, with 35,441 English UI strings in `i18n/en-US.json`; the
strings were searched for review, diff, finding, pull request and CI terms, then traced to the chunks that render
them. The Claude Code executable was read as Latin-1 and searched by identifier, as in
[the Claude Code comparison](../claude-code-binary-comparison.md). Nothing was run: no session, no inference, no
account data. Public documentation (`code.claude.com/docs/en/code-review`, `/ultrareview`) agrees with what was
found. Extracted source stays in the scratch folder, not git.

## How Claude reviews code

### The command

`/code-review [level] [target] [--fix] [--comment] [--max-findings N|all]`, alias `/review`, is a bundled
command in Claude Code. The level the user last typed is remembered and reused.

- Scope: the branch's committed work against its upstream, plus `git diff HEAD`, so uncommitted work is in
  scope. A PR number, branch or path replaces it.
- Levels: low, medium, high, xhigh and max.
  - Low: one read of the diff, no subagents, no verification, test hunks skipped, at most four findings.
  - Medium and high: eight finder subagents, up to six candidates each, then one verifier per candidate voting
    confirmed, plausible or refuted. Medium asks for precision, high for recall.
  - Xhigh and max: ten finders and a final "sweep for gaps" pass that looks only for what the list misses.
- Finder angles: a line-by-line scan, an auditor of removed code (what invariant did a deleted line enforce, and
  where is it now), a tracer of callers and callees, reuse of existing helpers, simplification, efficiency,
  "altitude" (is the fix at the right depth), and conventions from CLAUDE.md files, where a finding must quote
  both the rule and the line. Max adds language pitfalls and wrapper/proxy correctness.
- For some models the finder count scales with the diff: about one finder per 150 changed lines, two to eight.
- For Opus 5 at medium and high the fan-out is replaced by a short single-pass prompt marked as externally
  measured. Anthropic found the pipeline not worth its cost on its strongest model; the choice is per model.
- `--fix` applies the findings afterwards, skipping any that would change intended behaviour. `--comment` posts
  inline PR comments through a GitHub MCP tool or `gh api`, or one merge request note through `glab`.
- After the review it chains into a `/verify` skill when the diff has something that runs: the review checks
  the diff reads right, `/verify` that it runs right.

### The `ReportFindings` tool

Read-only and safe to run concurrently. Input is `{level, findings}`, at most 32 findings, most severe first.
Each finding has `file`, `line`, `summary`, `short_summary` (60 characters at most), `failure_scenario`,
`category` (a kebab-case slug such as `correctness` or `simplification`), `verdict` when a verify pass ran, and
`outcome` (`fixed`, `skipped` or `no_change_needed`) only when re-reported after fixes. The model calls it once
instead of printing the findings, and must call it again with outcomes whenever the findings are fixed later in
the session, because the UI's per-finding status updates only from that call.

### The desktop UI

- A transcript card: "Code review · high · 5 findings", findings grouped by file, and buttons Walk through in
  diff, Apply fixes and Re-run review. Once outcomes come back it reads "3 fixed · 1 skipped".
- Walk through in diff puts the findings into the diff pane as annotations with a "Finding 2 of 5" stepper.
- Apply fixes sends the open findings as a prompt and marks them fixing until outcomes arrive.
- A finding or comment whose lines changed since the review is shown as unanchored ("These lines changed after
  the review. Send the comment to Claude instead.").
- Comments on a selection can be queued and sent together.
- Pull requests: a bar with CI status, review threads that can be answered, replied to and resolved on GitHub,
  applying a reviewer's suggestion, auto-fix of failing CI, conflicts and review comments, auto-merge, and
  archiving the session and its worktree when the PR merges.

Ultrareview (a cloud fleet) and the Code Review GitHub App run on Anthropic's infrastructure and are billed per
review; neither can be reproduced locally.

## Drift at `3226d6e9b`

Already at parity: the review panel beside the chat (a turn, uncommitted changes, against a branch or one
commit) and line notes that join the composer as one attachment.

Missing:

- A command that runs a review. `/review` and `/security-review` were open in `CHECKLIST.md`.
- A typed findings tool, and any UI for findings: a card, annotations in the diff, outcomes.
- Notes or findings that notice their lines moved.
- Any pull request or CI view; `/watch-pr` was open.

## What to adopt

1. A `report_findings` tool with Claude's fields. The findings live in the call's metadata, so they stay in the
   session record with no new table. Drift also records the text of each finding's line when it is reported, so
   the panel can tell a moved line from a changed one.
2. `/review` as a built-in skill, so the transcript shows the command and a skill row rather than the prompt.
   Levels low, medium, high and max; finders grouped by diff size; verification by batch rather than one
   subagent per candidate, which is where Claude's pipeline spends most; `REVIEW.md` and `AGENTS.md` for
   conventions; `--fix` and `--comment` (through `gh`).
3. A findings card and a findings layer in the review panel: annotations, the stepper, Apply fixes, outcomes,
   Dismiss and Re-run.

Later: a `/verify` skill driving the change end to end (the browser pane covers most of it), and a PR bar with
CI status built with `/watch-pr`. Not adopted: ultrareview, the GitHub App service, review spend analytics and
auto-merge.

Prompts should be chosen per model family from the catalog, as base prompts are, once there is evidence for a
family; Drift starts with one pipeline.

## Other gaps, from strings only

Scheduled tasks and routines; `/batch` (plan a change, then 5 to 30 worktree agents each open a PR); automatic
verification through a managed dev server preview (`.claude/launch.json`); SSH sessions; grouping sessions by PR
state; Open, Copy path and Attach as context from the diff's file tree. None was traced further.
