# Tool benchmark: Drift, Claude Code and opencode

Measured 2026-11-18 on Kyle's machine (32 threads, NVMe, Windows), Drift at `a2f8611300` (`next/2.6.0`, release
`drift-engined`), Claude Code 2.1.147 (`~/.local/bin/claude.exe`, the agent loop Claude Desktop runs; the
desktop 4.23118 package had not downloaded its own runtime) and opencode 1.15.7 (`opencode-ai`'s `opencode.exe`).
No model and no credit was used: a local stand-in for the Anthropic Messages API scripted every tool call.

## Method

- **One script, three real agents.** The fake API reads each agent's own tool list from its first request and
  answers each step with that agent's spelling of the same operation (`Read{file_path}`, `read{filePath}`,
  `read{path}`). The time from the end of the reply carrying the tool calls to the arrival of the next request is
  the tool's cost as the user feels it: dispatch, permission, the tool, persistence, history rebuild and the next
  request. Startup is spawn (or session create) to first request.
- **Same conditions.** Each agent got its own fake home (`USERPROFILE`, `XDG_*`, `CLAUDE_CONFIG_DIR`), so none read
  Kyle's config. Permissions were all allowed. Every workspace was warmed with a full `rg` pass first and the agent
  order rotated per suite. opencode 1.15.7 runs shell commands in PowerShell on Windows, so its shell steps use the
  PowerShell spelling; Claude and Drift use Git Bash.
- **Realistic token counts.** The fake reports `input_tokens` as request bytes / 4 and refuses prompts over
  1M tokens with the API's `prompt is too long`, so compaction and overflow recovery are the agents' own.
- **Workspaces.** low: Drift's `src` (410 files). medium: a local clone of opencode (8,958). high: a sparse clone of
  Unreal Engine's `Engine/Source` (139k files, 1.6 GB). xhigh: the real UnrealEngine checkout (221k tracked
  files, read-only). projects: `Desktop/C++` (347k files, read-only). drive: `C:/` (1.8 TB used, read-only).
  Writes only ever went to scratch copies.
- **Where an agent lacks a feature, the others were still measured** and the row says so.
- The harness (fake API, adapters, suites, fixtures) is in the session scratch folder, not in git.

Numbers are medians of the repeats; "1st" is the first call where it differed. Output size is characters the
model receives, which is context and cost on every later request.

## Headline

1. **Drift's search is the fastest by a wide margin from 9k files up.** UE source: broad glob 0.26 s (Claude 1.9 s),
   rare grep 2.3 s (Claude 5.9 s), eight searches at once 4.9 s (Claude 13.1 s). On `Desktop/C++` and `C:/`
   Claude's 20 s ripgrep cap turns searches into errors with nothing returned; Drift answers in 0.6 to 15 s, or
   stops at 20 s with what it found and which folders it skipped. opencode has no cap and took 22.6 minutes for
   one grep of `C:/`.
2. **Drift's shell is the weakest part.** Any command its read-only list does not know (`sleep`, `export`, `:`,
   `exit 3`, `touch`, `npm test`, `cargo build`) takes a whole-workspace undo capture first: +250 ms on a small repo,
   +2.1 to 3.1 s on UE source, every time. The same rule makes such calls run one at a time: four parallel 1 s
   sleeps took Drift 5.0 s, Claude 1.1 s.
3. **Drift sends the most context.** Grep defaults to matching lines, outputs are inline up to 32 KB, repeat reads
   are sent again in full and nested `AGENTS.md` files ride along. After the same 22 search steps Drift's request
   was 317 KB, Claude's 190 KB, opencode's 226 KB; on UE source 940 KB against 640 KB.
4. **opencode's per-step git snapshot dominates everything it does.** 0.6 s a step on 410 files, 1 s on 9k,
   4.5 s on 139k; the first step waits 41 s (medium) and **13.7 minutes** (UE source). Off, it is 35 ms.
5. **Drift is the lightest and quickest loop.** 3 to 5 ms per trivial step (Claude 5 to 15, opencode 35 with
   snapshots off), a 10 ms session start, 112 MB peak engine memory on the UE search load (Claude 422 MB,
   opencode 2.3 GB).

## Loop, startup and fixed cost

| | Drift | Claude Code | opencode |
| --- | --- | --- | --- |
| Start to first request | 0.6 s engine once, then 10 to 30 ms per session | 0.42 s (0.9 to 1.4 s in big repos) | 1.1 to 3 s |
| Trivial step (3-line read) | 3 to 5 ms | 5 to 16 ms | 600 ms (35 ms snapshots off) |
| Per-step cost at step 200 (2 MB history) | 19 ms | 15 ms | 760 ms |
| First request: tools + system | 22 tools, 38 KB schemas, 5 KB system, 44 KB total | 26 tools, 71 KB, 26 KB, 104 KB | 10 tools, 22 KB, 10 KB, 33 KB |
| Peak memory, UE search load | 112 MB (36 MB idle) | 422 MB | 2,315 MB |
| Todo update (5 items) | 2 ms | 30 ms (5 `TaskCreate` calls) | 590 ms |
| Subagent that answers at once | 8 ms | 11 to 38 ms | 1.2 s |
| Three subagents at once | 18 ms | 15 ms | 2.4 s |
| Skill load | 4 ms, 2.9 KB in the result | 6 to 196 ms, body sent as its own message | 650 ms |

Drift's per-step cost grows with history because every step reloads the whole transcript from SQLite and decodes
each part (`request_window` in `session/turn/request.rs`): 5 ms at step 0, 19 ms at step 200 (2 MB), about 125 ms
once a 10 MB tool input sits in history. Claude stays at 12 to 15 ms because it keeps the conversation in memory.

## Search

Median per call; output in characters. `-` is a gap: opencode's `grep` has no output mode, case or context flags.

| Workspace (files) | Operation | Drift | Claude Code | opencode |
| --- | --- | --- | --- | --- |
| low (410) | glob broad | 9 ms, 2.4 K | 61 ms, 2.3 K | 642 ms, 12 K |
| | grep rare | 17 ms | 76 ms | 657 ms |
| | grep common, default mode | 7 ms, 19 K | 77 ms, 6 K | 28.9 s, 7 K |
| | 8 searches at once | 30 ms, 33 K | 117 ms, 4 K | 911 ms, 23 K |
| medium (9k) | glob broad | 27 ms | 198 ms | 912 ms |
| | grep rare | 155 ms (1st 3.1 s) | 390 ms | 1.2 s (1st 4.7 s) |
| | grep regex with include | 18 ms, 27 K | 303 ms, 10 K | 5.3 s, 7 K |
| | 8 at once | 352 ms, 39 K | 931 ms, 23 K | 6.3 s, 42 K |
| high, UE source (139k) | glob narrow / broad | 245 / 261 ms | 721 ms / 1.9 s | after a 13.7 min first snapshot |
| | grep rare | 2.3 s | 5.9 s | |
| | grep regex with include | 0.98 s | 2.7 s | |
| | count | 0.73 s | 3.2 s | `-` |
| | 8 at once | 4.9 s, 162 K | 13.1 s, 33 K | |
| xhigh, UE root (221k) | glob narrow / broad | 416 / 429 ms | 1.4 / 3.5 s | 2.0 s / 214 ms |
| | grep rare | 3.9 s | 9.3 s | 10.1 s |
| | grep regex with include | 0.47 s | 5.2 s | 5.9 s |
| | 8 at once | 6.7 s | 20.1 s, 3 of 8 timed out | 29.3 s |
| | grep common, default mode | 16 ms (stops at 250 lines) | 13.0 s | error: invalid ripgrep output |
| projects (347k) | glob narrow | 589 ms | **timed out at 20 s** | 1.2 s |
| | glob broad | 583 ms | 16 s | 477 ms |
| | grep rare | 5.9 s | 16.3 s | 21.7 s |
| | 8 at once | 9.3 s | 20.1 s, timeouts | 36.8 s |
| drive `C:/` | glob narrow | 9.2 s (1st 15.3 s) | timed out | 98.5 s, access-denied noise |
| | glob `**/*.dll` | 9.1 s | timed out | 0.3 s |
| | grep, no filter | stops at 20 s, says what it skipped | timed out | 22.6 min |

- Claude's Grep and Glob run `rg` with a 20 s limit and return only an error when it trips; partial matches are
  thrown away. Drift's stop at 20 s keeps what it found and says so.
- Claude defaults Grep to file names (sorted by mtime, so it stats every match); Drift defaults to matching lines
  and stops at 250. Drift answers "where is X" in one call, Claude often needs a second call to see the lines,
  but Drift's answer is 3 to 5 times larger.
- Claude saves any grep or shell output over about 20 KB to a file and sends a 2 KB preview with the path.
- opencode's common-word grep is pathological at every size (28 s on 410 files) because it gathers every match
  before sorting by modification time.

## Files

From the files suite in the 410-file workspace; correctness is checked on disk afterwards.

| Case | Drift | Claude Code | opencode |
| --- | --- | --- | --- |
| Read 150 lines | 4 ms | 12 ms | 658 ms |
| Read 15k-line file (840 KB) | first 2,000 lines, 64 KB | **refused: over 256 KB**, use offset | 2,000 lines, 56 KB |
| Read UE `Actor.h` (5,077 lines, 255 KB) | 1,305 lines (64 KB cap) and where to go on | whole file, 280 KB | 2,000 lines |
| Same file read again, unchanged | full content again | 93-char "file unchanged" note | full content again |
| 50 MB log, default / deep offset / tail | 4 / 20 / 22 ms | refused / 39 / 35 ms | 657 ms / 1.0 / 1.1 s |
| 5 MB single line | cut at 2,000 chars | refused | cut |
| Image / PDF | 9 / 4 ms | 65 / 66 ms | 711 / 669 ms |
| PDF pages 1-5 of 40 | 4 ms | **fails: needs `pdftoppm`**, not on Windows | whole PDF attached |
| UTF-16 text | read as text | read | **refused as binary** |
| Directory | lists it | refused | lists it |
| Write 2 KB / 10 MB new file | 12 ms (145 ms first in a workspace) / 1.0 s | 25 / 153 ms | 745 / 991 ms |
| Overwrite a file never read | refused | refused | **overwrote it** |
| Edit, exact | 18 ms | 15 to 57 ms | 889 ms |
| Edit a file never read | applied | refused | applied |
| Edit past line 2,000 of the 840 KB file | applied | **impossible**: it cannot be read whole | applied |
| `replace_all` 20,000 hits (300 KB file) | 1.3 s | impossible, same reason | 57.6 s, then **37 s every later step** |
| Old text lacks trailing spaces | applied | failed | applied |
| Old text uses spaces, file uses tabs | failed | failed | applied, file indented with spaces |
| Straight quotes and `-` against curly quotes and a dash | applied | failed | failed |
| LF old text in a CRLF file | applied, CRLF kept | applied, CRLF kept | applied, CRLF kept |
| File changed outside after the read: edit | applied, outside change kept | refused until read again | applied |
| ... then a whole-file write | refused until read again | refused | **wrote over the outside change** |

Writes and edits cost the same at every repository size in Drift and Claude (Drift 9 to 22 ms after its first
write in a workspace, which costs 140 to 165 ms to set up the undo store; Claude 11 to 29 ms). opencode pays its
snapshot on each: 0.7 s on low, 1.0 s on medium, 4.4 s on UE source.

## Shell

| Case | Drift | Claude Code | opencode (PowerShell) |
| --- | --- | --- | --- |
| `echo hi` | 41 to 47 ms | 71 to 75 ms (1st 0.85 s) | 330 to 850 ms |
| `export X=1`, `:`, `exit 3`, `sleep 0.2` overhead | **+250 ms** low, +230 ms medium, **+2.1 s UE source** | none | none |
| `touch file` on UE source | 3.1 s | | |
| 4 x `sleep 1` in parallel | **5.0 s, one at a time** | 1.1 s | 1.9 s |
| 10 MB of stdout | 0.7 s, 33 KB inline | 0.1 s, preview + saved file | 60 s, preview + saved file |
| `cat` of a 50 MB file | 0.19 s, 33 KB | 0.11 s, preview + file | 2.1 s |
| 2,000 stdout/stderr lines interleaved | 0.35 s, order mixed | 0.12 s, order kept | 1.1 s |
| Timeout 2 s on a 30 s command | moved to background at 2.3 s | killed, exit 143 | killed at 2.8 s |
| Start in background | **1.27 s** (waits 1 s for an early failure) | 18 ms | no background |
| Wait for it | 0.9 s, its output | 2.2 s | |
| `cd` / `export` carried to the next call | no / no | no / no | no / no |

The +250 ms to +3 s is the undo capture Drift takes before any command `command::reads_only` cannot prove
read-only; the same verdict makes the call run alone (`session/turn/calls.rs`). Drift's reader list has 53
programs and 9 git subcommands. Claude's `readOnlyValidation.ts` also accepts `sleep`, `sort`, `uniq`, `seq`,
`tr`, `nl`, `paste`, `column`, `test`, `expr`, `false`, `id`, `ps`, `netstat`, `lsof`, `xargs` with safe
targets, `fd`, `jq`, `find` with flag checks and `git branch`/`remote`/`tag`/`config --get`/`stash list`, each
with per-flag validation.

## Streaming tool execution

Three greps over UE source, the tool calls streamed 1.5 s apart as a model would emit them:

| | Drift | Claude Code | opencode |
| --- | --- | --- | --- |
| All three at once | 9.3 s | 20.1 s | 21.4 s |
| Paced 1.5 s apart (time after the reply ends) | **5.8 s** | 20.0 s | 17.8 s |

Drift starts each call as its block completes. Claude 2.1.147's `StreamingToolExecutor` is behind a remote flag
and did not run here; its three greps ran close to one after another (raw `rg` takes 6.1 s each).

## Web fetch

| Case | Drift | Claude Code | opencode |
| --- | --- | --- | --- |
| 5 KB page | 3 ms, raw markdown | 52 ms, then cached | 558 ms |
| 3 MB HTML | 200 ms, first 33 KB and where to go on | 881 ms, then cached 15 min | 6.8 s |
| 1 MB JSON | 5 ms, 33 KB | 10 ms | 626 ms, saved to a file |
| `localhost` or a one-word intranet host | fetches (asks first for private hosts) | **refused**: host needs a dot | fetches |
| `http://` | as given | upgraded to `https://` | as given |

Claude's WebFetch never shows the page to the main model: a second model call (Haiku in production) reads it and
only that answer comes back. Here that call was free and instant; in use it adds a request, seconds and cost to
every fetch, and the main model cannot quote the page.

## Context limits

- 40 reads of 64 KB each (about 650k tokens). Drift clears old tool output and sent 1.5 MB at the end, no
  compaction needed in a 1M window. opencode sent all 2.3 MB. Claude Code treats Sonnet 4.6 as 200k and ended the
  run with `blocking_limit` at about 190k tokens instead of compacting.
- A 4.5 MB write pushes the prompt past 1M tokens. Claude ends at once (`prompt_too_long`). opencode compacts, then
  stops. **Drift starts a compaction three times, but each summary request carries the same oversized history and
  is refused, so the turn makes 7 refused requests and ends in an error.** The summary request must drop or cut
  oversized tool inputs and outputs first.

## Features only one or two have

| Feature | Drift | Claude Code | opencode | Measured |
| --- | --- | --- | --- | --- |
| Browser | `browser` tool, Chrome/Edge/Firefox/Camoufox | Claude in Chrome extension (not installed here, needs a signed-in Chrome) | none | Drift: warm open 0.49 s, snapshot 8 ms, click/fill 0.34 s, screenshot 42 to 77 ms, 20k-element page load 0.56 s, its snapshot 1.4 s and 50 KB. First open in a fresh profile timed out at 30 s. **Bug: `open` right after `stop` fails** ("Target.createTarget: Failed to open a new tab") |
| Worktrees | create/list/remove/session_move | Enter/ExitWorktree | none | Drift create 0.34 s, remove 0.19 s; Claude enter 0.57 s, exit 0.47 s. Both fail on opencode's repo with Windows long paths |
| Background shell | `bash background` + `process` | `run_in_background` + `TaskOutput` | none | above |
| Notebook edit | none (edit the JSON) | `NotebookEdit` 17 ms | none | gap for Drift |
| Scheduling | none | `CronCreate`/`ScheduleWakeup` 5 ms, session-only | none | gap for Drift |
| Plan mode tool | agent preset | `EnterPlanMode` 10 ms | agent preset | |
| Code review findings | `report_findings` | `ReportFindings` (desktop) | none | |
| Web search | providers with keys (not run: credits) | server tool (not run: credits) | none | |
| Questions | `question` | `AskUserQuestion` | none | |
| Image save, thread read | `save_image`, `read_thread` | none | none | |
| Loop guard | pauses after 3 identical steps | none | `doom_loop` asks after 3 | |

## What to do in Drift, by measured impact

1. **Widen `command::reads_only` to Claude's coverage** (with per-flag checks) and stop treating a pure
   environment or flow builtin (`export`, `:`, `exit`, `true`, `test`, `[`) as a possible write. This removes
   +2 to 3 s per command in large repos and lets common parallel commands run together.
2. **Make the pre-command undo capture cheap in big repos:** reuse the last capture when the index and the
   watcher say nothing changed, or capture after the command by diffing against the index, rather than before
   every unknown command.
3. **Cut oversized parts out of the compaction request** so an over-window turn recovers instead of looping on
   refused requests.
4. **Fix browser `open` after `stop`** (wait for the old browser to finish closing, or drop its connection) and
   look at the 30 s first launch in a fresh profile.
5. **Answer an unchanged re-read with a short note**, as Claude does, and consider saving outputs over about
   20 KB to a file with a preview, for grep and shell alike.
6. **Keep the request window in memory** between steps instead of reloading the transcript from SQLite each
   time; it is 19 ms at 2 MB and 125 ms at 10 MB.
7. **Shorten the background start**: return at once and report an early exit in the next result, as Claude does,
   instead of waiting 1 s.
8. Smaller: keep stdout and stderr in arrival order per line; consider files-first grep output for very broad
   patterns; a `NotebookEdit` equivalent.

## Where Drift already leads

Search speed and its partial-result behaviour on huge trees, loop overhead, memory, streaming execution, PDF
page reads on Windows, UTF-16, editing large files, typographic-quote and dash tolerance, edits that keep an
outside change while refusing a stale whole-file write, fetch of local hosts with raw content, and worktree,
browser and background-process tools in one engine.

## After the fixes

Measured again on the same fixtures and mock, Drift only, after the changes on `next/2.6.0`:

| Case | Before | After | Change |
| --- | --- | --- | --- |
| `:` or `export` on UE source (139k files) | 2.1 s | 42 ms | `sleep`, `export`, `test`, `sort`, `jq`, `fd`, listing forms of `git branch`/`tag`/`remote`/`config`, `--version` and more now count as reading, so no undo capture |
| Writing command on UE source, steady | about 2.1 s | 0.88 s | one `git status -uall` walk against a HEAD commit of the last capture replaces `diff-files` plus `ls-files --others` (870 ms, no fscache) |
| Four parallel 1 s sleeps | 5.0 s | 1.1 s | read-only now, so they run together |
| `export` (410 files) | 289 ms | 49 ms | |
| Background start | 1.27 s | 0.27 s | settles 250 ms in bash, 1 s in PowerShell, which takes up to 700 ms to start |
| Interleaved stdout and stderr | order mixed | exact order | bash runs `exec 2>&1` first |
| Step 199 of 200 (2 MB history) | 19 ms | 13 ms | parts load in message order from the index; ordering by `p.message_id` sorted every row's JSON |
| Last request, 200-step read loop | 2.0 MB | 629 KB | unchanged re-reads answer with a note while the earlier result is still sent |
| Last request, 22 search steps (low) | 317 KB | 268 KB | grep's default page stops at 20 KB; shell, fetch and MCP keep 10 KB of each end past 20 KB. Claude: 190 KB |
| 4.5 MB `write` over the window | 7 refused requests, error | 1 refused request, compacts, finishes | the summary request cuts long strings in tool inputs and texts over 40,000 characters |
| Browser `open` right after `stop` | "Failed to open a new tab" | new browser in 0.72 s | a browser asked to close counts as ending at once |

- The first writing command in a fresh session still pays the first capture (about 0.9 s on 410 files, 3.3 s
  on UE source). With `seq` now read-only, the shell suite's first capture moved from the 10 MB output step
  (696 to 87 ms) to the mixed-output step (351 ms to 1.17 s): the same one-time cost on another step.
- The 30 s first browser launch did not happen again in four fresh data dirs (0.47 to 1.4 s); Chrome itself
  starts on a new profile in about 0.3 s. It happened while three agents ran heavy suites at once, so the launch
  timeout is unchanged.
- The request window is not cached in memory: with the sort gone, a step at 2 MB of history costs less than
  Claude's (13 ms against 17 ms), and a cache would have to follow every writer of a session's parts.
- No `NotebookEdit`: a notebook is JSON that `edit` and `apply_patch` already change exactly.
