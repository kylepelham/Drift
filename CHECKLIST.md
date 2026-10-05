# Checklist

Current state of the engine rewrite on `next/1.4.0-engine`. The plan and every
decision behind it are in `docs/engine-rewrite.md`. Tick items here as they land;
change the plan there when a decision changes.

## M0: skeleton

- [x] Cargo workspace with `crates/drift-engine`, linked into `src-tauri`
- [x] axum on loopback inside the Tauri process, `/health`, `/openapi.json`
- [x] WS hub: `seq`, ring buffer, `cursor` replay, `resync`
- [x] SQLite store with versioned migrations in `drift.db`
- [x] Generated TypeScript client wired into `src/engine/`
- [x] Perf baselines recorded against the opencode engine

## M1: vertical slice

- [x] Anthropic adapter: API key, subscription OAuth, streaming, tool calls, thinking
- [x] Turn loop
- [x] `read`, `edit`, `write`, `bash`, `glob`, `grep`
- [x] Exact-match edit with closest-region miss reporting
- [x] Permissions end to end
- [x] Snapshot before writing tools
- [x] Persistence
- [x] One `drift.db` connection shared by engine and shell
- [x] Real turns against Anthropic through `drift-engined` (read, edit with ask, bash with ask)
- [x] Drift UI against the new API (adapter over the legacy store shapes; engine-side verified, shell run pending)
- [x] Failure paths: terminal validation, atomic admission, replay eligibility, single-flight refresh (also once on a refused sign-in, which otherwise says it expired), hydrate cursor safety, full listings
- [x] M1 sign-off corrections: resolved paths, refused unsafe writes, process-tree stop, replay eligibility, auth fence
- [x] Conformance suite with recorded Anthropic fixtures (`tests/conformance`)

## M2: breadth

- [x] OpenAI Responses, Codex OAuth, `apply_patch` profile
- [x] Gemini (adapter only; not yet exercised against the live API)
- [x] OpenAI-compatible generic with xAI, Z.ai, OpenRouter, LM Studio and Ollama presets
- [x] MCP through rmcp: stdio, HTTP, approval (reconnect and reload: M3)
- [x] `todowrite`, `question`, `webfetch`
- [x] `skill`
- [x] Read-only tool calls run concurrently
- [x] Questions (blocking; async variant is M3)
- [x] Formatter hooks
- [x] Config loading: `drift.json`, agents, commands, skills, instruction files

## M3: tree and lifecycle

- [x] Foreground `task` subagents (abort cascades, no nested delegation, sidebar only while active)
- [x] Spawned threads: `/spawn <instruction>` copies the finished conversation into a linked, independent thread and starts it on the instruction (no draft, no review); the model cannot spawn; `read_thread` for spawned threads
- [x] Typed worker execution-mode resolver: explicit override, agent default, foreground fallback; no chat-keyword switches
- [x] Background `task` launch receipts and bounded engine-owned jobs, independent progress from the main session
- [x] Durable terminal results and idempotent parent completion delivery at safe provider boundaries
- [x] Parent-idle survival, worker/session Stop and restart interruption handling without automatic effect replay
- [x] Attributed worker permission/question waits, parent-scoped task output/stop API and generated lifecycle events
- [x] Async-worker conformance: engine-level gates (concurrent progress, out-of-order results, bounded slots, Stop while idle, single-worker stop, attributed asks, restart) and HTTP/WS conformance against the fake provider (progress events, parent carries on, single delivery, cursor replay, idle Stop wakes nothing)
- [x] UI task views: engine store `tasks` from `GET /sessions/{id}/tasks` and `task.updated`; transcript task rows follow the worker, not the launch receipt; Background tasks dock with state, current tool, Stop and open
- Agent-loop review at b93cbc9 (`docs/research/agent-loop-review-b93cbc9.md`):
  - [x] Undo/redo keep a path whose change chain another edit broke
  - [x] `apply_patch`: read-before-write for every existing target and move destination, an ask per path, whole patch checked before any write, rollback on a failed write
  - [x] SSE decoding keeps characters split across reads whole
  - [x] A worker cut off at its output limit is `incomplete`, not `replied`
  - [x] Malformed data URLs refused at admission; full mentions count as reads; mention reads bounded; steering judged against the running model
  - [x] Error and token bodies read within size and time bounds
  - [x] A session reads its own spilled output without asking
  - [x] Runners and installers approved only exactly; deny rules see past assignments and PowerShell aliases
  - [x] Undo history owned by workspace id; moves keep it; pruning parses outside the database lock
  - [x] Route-configurable timeouts, local routes 600 s; Deny and stop scope stated; OpenAI `prompt_cache_key`; `require_git(false)` for the large-file walk; stale concurrency doc fixed
- Worker review at 06a5e0c (`docs/research/worker-review-06a5e0c.md`):
  - [x] Each worker has its own token, registered before planning or queueing; Stop reaches it queued, starting, planning and running; checked again after a slot is granted
  - [x] Stop and prompt admission share one fence; deliveries carry the launch-generation scope through every wait into it
  - [x] Durable per-owner Stop count and per-task launch generation; restart recovery suppresses results launched before a Stop
  - [x] One claim-and-acknowledge delivery path: `delivered` set only in the write that saves the prompt or call result; task_output and foreground included; foreground recovery writes into its own call
  - [x] Worker plan (prompt, tools, limits, workspace, model) fixed at admission; only credentials refreshed at start
  - [x] Task and child session created or reused in one transaction; replays return the mode's own result
  - [x] `apply_patch` writes through staged copies, restores the failing step too, names files it could not restore; only NotFound is absence
- Worker follow-ups:
  - [x] Owed deliveries retried on the parent's job end and on repairs (credentials, session model/agent, agent overrides), with the reason exposed; a trigger racing a held claim is kept by the attempt; no timer loop
  - [x] Results Stop suppresses are held, not delivered, and ride along with the next user prompt, once
  - [x] `task_output` is background-only; launching calls own foreground (and failed-launch) results through their save
  - [x] Staged writes: Windows `ReplaceFileW` keeps the ACL or fails; delete-sharing refusal leaves the file; hard links documented; only recorded engine-named staging files cleaned
  - [x] New comments one line each
  - [x] Startup recovery settles recorded replacement pairs: a stranded backup is moved back, a failed restore keeps both files and the record; pairs recorded in one statement, forgotten in one short transaction after file I/O
  - [x] `edit`, `write` and undo/redo go through the staged writer; `write` treats only NotFound as a new file
  - [x] Held results ride along with a later permitted delivery, in the same write
  - [x] Undo and redo are all or nothing: a failed write puts back the files already changed, so a retry reports nothing as kept
  - [x] A replacement is marked `swapped` once its swap succeeds, so a stale backup is removed, never restored over a deliberate deletion
- [x] Fork: bounded (`atMessage`) and active (stable history, in-flight turn left out)
- [x] Move with busy guard (subagents move along, branches stay; retarget refuses while running)
- [x] Per-action models from Settings > Agents: title (small model default, generated in the background), compaction, subagent pins
- [x] Compaction with recovery: automatic (meter threshold, Settings off switch, stops after 3 failures), overflow compact-and-retry, `/compact`; summary plus a 2-turn tail of a quarter of the compaction point (2k to 15k), nothing deleted
- [x] Retry with model switch: `session.retry` drives the retry notice, `POST /sessions/{id}/retry` switches a waiting turn's model at once
- [x] Revert (undo/redo with files, across subagents; the next prompt commits it). Diff: per-call diffs in tool metadata; no session diff endpoint until something consumes one
- [x] Shell timeout: Settings value pushed to the engine, model `timeout` wins, badge metadata while running and on expiry
- Native agent loop gaps (before async workers, in order):
  - [x] Undo restores only attributable paths and keeps later edits
  - [x] Snapshots: per-workspace lock, 10MB limit, retention and prune
  - [x] Shell-aware approvals: per-command decisions, no widening to the program name
  - [x] Secret files ask to be read inside the workspace, grep withholds them, examples exempt; `.git` and binaries skipped by search
  - [x] Anthropic conversation cache breakpoints (API key, subscription, Anthropic-dialect gateways)
  - [x] OpenRouter Claude caching: OpenRouter is a catalog provider; Claude models get per-block `cache_control` (system and the last two user messages) and cache usage is read; verified with a recorded exchange against a local stand-in
  - [x] SSE error classification and bounded, cancellable, `retry-after`-aware backoff
  - Review of items 1 to 6:
    - [x] Byte-exact shadow snapshots (attributes overridden, existing repos migrated)
    - [x] Changes seen during a command are unattributed and never undone
    - [x] Oversized tracked files leave the shadow index; reported as unrecorded, not deleted
    - [x] Snapshot cost measured; no tree reuse without a reliable unchanged signal
    - [x] File-writing redirections need exact approval (bash and PowerShell)
    - [x] Permanent quota errors never retry; oversized `retry-after` cannot panic; failed jobs release the session; cap after jitter
    - [x] Bounded shell capture; inherited pipes and background descendants
    - [x] `.envrc`, `cd` chains, opaque-construct docs, periodic prune (OpenRouter caching checked, left open above)
  - [x] Shared HTTP client with connect, header and stream-idle timeouts; Stop cancels while waiting
  - [x] Thinking budget within the output limit; max-tokens endings surfaced; unexecuted calls settled
  - [x] Configurable step limits and repeated-call intervention
  - [x] Shared tool-output limits with full-output artifacts (read, list, MCP, shell)
  - [x] Engine-owned steering and queueing at safe boundaries
  - [x] File discovery and @ expansion, every read through `tool::read_ask`
  - [x] Attachments validated against model capabilities; no silent drops
  - [x] Explicit denial and stop feedback; parent-to-worker permission inheritance
- [x] Per-session runtime config snapshots: a turn's `Plan` holds its config and the tool objects it offered (MCP clients included) until it ends; call-time config reads use the snapshot
- [x] MCP reconnect and reload: watched connections reconnect with capped backoff under the server's generation, planning waits up to 2 s for connects under way, an approved save reconnects at once while running turns keep their client
- [x] MCP lifecycle coordination: one lock over row writes, generation bumps, connect starts and publishes; bounded `initialize` and `tools/list`; superseded or abandoned attempts cancelled with their process tree; backoff kept until a connection proves stable
- [x] MCP tools in running turns: per-server slot so captured tools follow a reconnect of the same definition, disable and remove end calls and refuse captured tools, read-only calls retried once after a lost connection, calls that may have changed something never replayed
- [x] Async questions: default async, answers saved as a `clarification` prompt through normal admission (joins, starts, or only saves after Stop), card closes after the save, idempotent resend, engine-only parts refused from clients; per-request decision lock, submission replay settled inside the admission write, store-backed resend check that survives a restart, `question.result` frames for socket replies
- [x] Reasoning level (missed at M1): picker variants from models.dev `reasoning_options` (as opencode), mapped per wire after checking current provider docs (Claude adaptive effort or budget, Gemini level or budget, OpenAI effort, OpenRouter object, `reasoning_effort` elsewhere); variant saved per session so engine-started turns keep it (worker deliveries, question answers, subagents, branch seeds, retry with a model switch)
- [x] Bedrock (SigV4 or Bedrock API key, env and profile credentials, event-stream replies, Claude only) and Vertex (service account or ADC token, Claude and Gemini publishers); xAI and Z.ai as compatible presets. Verified against local stand-ins, not live accounts

### UI completion audit (`docs/research/ui-completion-audit-bc2a8cd.md`)

- [x] Composer agent selection runs the session as that agent (prompt `agent`, validated, saved at admission); sessions and messages keep the agent they ran as
- [x] MCP manager, registry installs and approval prompts on the native `/mcp` authority only; editor limited to fields the engine runs
- [x] Agent Settings apply what they show: overrides carry prompt, model, steps and tools (applied natively), any other field refused; editor projects the native agent
- [x] Model-family prompt editor states it is not applied and is read-only (no native family prompts)
- [x] Native base-prompt override (global or per model family) so the Prompts editor can save again (`/prompts`; a family's own wins over All models; the shared rules always follow)
- [x] Permission rules editor in Settings: list, add, reorder and delete the global rules (kind, pattern, allow/ask/deny), and the workspace's "always" grants with a way to revoke them (`/permission-rules`, checked after drift.json)
- [x] Archive and restore (sidebar, `/archive`, Archive dialog) archive in the engine first, with its Stop, then update the shell record
- [x] Bounded fork reachable (Fork from here on a finished reply); a resent prompt keeps its submission id until the engine answers for sure
- [x] Jev tool routing removed from Settings (toggle, state module and status polling)
- [x] Review fixes: MCP secrets never leave the engine (names only, write-only values), rename and add refuse a taken name, the archive purge deletes only what the engine still has archived, a prompt for another agent or level gets its own turn
- [x] Review follow-ups: captured MCP tools refused on a redefined reconnect, approval tied to the reviewed hash, unnamed and cleared variants hash apart, the composer shows the session's saved agent and level, legacy agent overrides save again
- [x] Second review: a follow-up is judged against the running turn by effect (agent by name, level by the reasoning it resolves to on the running model)
- [x] Second review: a prompt for another agent or level is queued durably and answered at once (202, `session.queued`), starts when the turn's step ends or the engine restarts; later prompts join or replace it; Discard and Stop return it, and nothing starts after a Stop
- [x] Second review: the composer shows a waiting prompt (who it runs as, or why it could not start) with Discard, and puts any prompt the engine gives back into the draft
- [x] Second review: composer picks are unsent edits cleared by an accepted send; a prompt names agent and level only when they change the session's, never null for a level the model lacks; the model falls back to the session's before the global default
- [x] Second review: the MCP approval hash is keyed (HMAC-SHA256, key kept by the engine); approvals under the old unkeyed hash carry over
- [x] Second review: a reconnected MCP tool is judged by name, input schema and safety hints, not its wording
- [x] Third review: a queued start lands only if every row it read still waits, so a prompt handed back never runs
- [x] Third review: a model change sent mid-turn waits for a turn on that model like an agent or level change; the composer shows the waiting model
- [x] Third review: a turn hands over to what waits only after its first request, so a result or answer arriving meanwhile gets its reply
- [x] App test pass: a model, agent or level sent mid-turn is followed by the running turn from its next request, as in opencode; the durable queue, waiting row and Discard are removed
- [x] App test pass: MCP approval removed (engine gate, route, hash, toast, buttons); rows are delete, edit, disconnect, enabled
- [x] Independent audit at ac1ab72: undo point save failures put files back; oversize writes refused and failed change records put back or reported; undo ordered by when writes finished (its queue findings went with the queue)
- [x] MCP OAuth sign-in for streamable HTTP servers: needs/has sign-in on the status, browser sign-in through a loopback callback, tokens in the keychain, sign-out; renames carry it, URL changes and removes forget it
- [x] MCP 2026-07-28: probe then fall back to the handshake, era kept per server (re-probed on save or refusal), stateless HTTP holds nothing open, tool lists re-read past `ttlMs` at planning, `input_required` declined, era on each row
- [x] MCP registry rebuilt: GitHub's curated registry popular-first with logos, stars and local ranked search, official registry matches appended; install sheet per run option (remote http/sse, npx, uvx, docker) asking only what the entry leaves open, secrets kept out of arguments, sign-in opened on install
- [x] Checks once per step over everything it wrote, in parallel within one budget, unchanged output not resent, fixers announced; a project's own check and formatter commands, and formatter programs installed in it, run only once allowed (each command answered on its own; always kept per workspace as the list of allowed lines)
- [x] MCP sign-in: typed 401/403 detection, pre-registered apps (`oauth: { clientId, clientSecret, scopes }`), SSE servers, failed sign-ins reported; loopback-callback limit on remote devices documented
- [x] Undo history across overlapping workspaces merges chronologically by canonical physical path, with independent snapshot owners for the original and final endpoints; full undo/redo, moving the marker, pruning, failed-marker rollback and broken-chain preservation are covered by end-to-end tests.
- [x] Concurrency follow-ups: atomic canonical-path reservations cover overlapping trees and outside-workspace writes; undo acquires one deduplicated path set and can cancel while waiting; Windows checks join a job before execution and cleanup waits for every job member; fork-finalization errors run partial-copy cleanup without masking the original error; snapshot-only parts are restored unless an explicit-removal tombstone forbids it.
- [x] Review follow-ups: undo holds all affected locks through marker commit or rollback; stopped checks are killed and reaped before history cleanup; whole-workspace fixers exclude file writers; a fork with a missing selected message fails and deletes its partial copy; long-line reads keep bounded buffers and check Stop per chunk; approval previews and execution share file locks; streaming gaps reconcile through HTTP and fuller snapshot prefixes survive live revisions; new paragraph comments reduced to one line and `files_for` documentation restored.
- [x] Fourth pass review: patch diffs kept under `fileChanges` beside undo's `changes`, checked through a real turn; deny rules come before kept "always" grants, which can be listed and revoked per workspace and go when the workspace is forgotten; a bare name linking out of the workspace asks; `filePath` only for file tools
- [x] Gaps against opencode, fourth pass: native file tools reach the UI's rows, diffs, file actions and citations; edit, write and apply_patch answer in one line with the diff in metadata; Git Bash found beside git, per-user and Scoop installs, `DRIFT_SHELL`; reading shell lines inside the workspace and workspace edits run without asking (drift.json, `.drift`, VCS internals and secrets still ask); "always" holds for the workspace across sessions and restarts and answers the asks it covers; command arguments split like a shell with any `$N`, `` !`line` `` runs as a checked call, `@path` is a mention; drift.json `instructions` take globs, absolute paths and `~/`
- [x] Third review follow-ups: a wrap-up runs no call even when the server ignores `tool_choice: none`, ends `limit` (migration 32) and reports a subagent's write-up as incomplete; repeated call ids in older sessions go out unique; webfetch's redirect note says the approved site; Gemini sampling from 2.5 on; read suggestions need three characters; ChatGPT titles on `gpt-5.4-mini`
- [x] Third review pass: call ids unique per session (engine ids when missing or repeated, migration 31) and Anthropic-safe on the wire; broken-JSON calls replayed with their parse error, wrong-case tool names run; a ChatGPT sign-in sees only Codex models with Codex limits, one-shots run at the weakest level with thinking room; a long single turn keeps its newest steps and its prompt verbatim when compacted; base prompt covers dirty worktrees, `<system-reminder>` and final answers; opencode's default reasoning, verbosity, thinking and per-family sampling; webfetch as a browser with a Cloudflare retry, redirects only within the origin; Windows PowerShell 5.1 described as itself; the last allowed step (and a repeat loop) writes up instead of losing work; agent sampling fields ignored with a warning; read suggests near names; skill lists its files; Z.ai keeps thinking
- [x] Second review pass: mid-turn prompts cannot pick a broken agent; a prompt steered into a command's turn returns the turn to the session's agent and model; legacy plaintext credentials merge into an existing encrypted store; Settings agent rules resolve last-match like agent files; broken subagents are not offered to the model
- [x] Review of 1a24e3720: approved searches cover their files outside the workspace and in scratch unless a rule says otherwise, with rules compiled once per search; empty signed Gemini text replayed only to Gemini; one-shot requests forbid tool calls on every wire; a broken agent or override refuses only itself, flow-map permissions read in opencode's last-match order; command agent and model last one turn and subagents always delegate; `task` has no model parameter; the credential ACL covers only the file; opaque rescaled images go JPEG first
- [x] Native agent issues and parity gaps: explicit rules restrict default-allowed reads, searches, skills, tasks and read-only MCP; Gemini sends raw schemas and keeps call signatures on their own parts (migration 30); one-shot summaries need a clean end; steering and retries stay on the admitted config generation; the credential file fallback is encrypted and private; BOM and CRLF kept through every write; agents keep permissions and a default variant (sampling and provider options refused); commands keep agent, model and subtask, and skills run as commands through the permission check; oversized returned images are scaled under provider limits or dropped with a reason
- [x] Review against opencode: a resumed socket brings the UI back online; writers of one file take turns across sessions (`tool::lock`); skills from `~/.agents/skills`, `~/.claude/skills`, every parent to the repo root and `skillPaths`, at any depth; forks copy page by page in SQLite and stop where the running turn began; a mid-stream snapshot shows the open part, deltas carry offsets; PDFs go whole to models that read them; file approvals show the diff, the card has deny-and-stop and a note; the reply's leading reads start while it streams; `read` pages through files over 10 MB
- [x] Follow-ups, third pass: formatter fingerprint keeps 16 bytes, covers peers and the project's plugins, runs once per write off the runtime with a bounded cache; grep stops past 2000 matches; read-only trust for an MCP server is a settings switch tied to its definition (`readOnlyMcp` removed); removed or renamed servers free their tool names
- [x] Follow-ups, second pass: formatter approval hashes the package's code and dependencies; `readOnlyMcp` vouches by name and what the server runs; MCP tool names kept once given; grep lists the first 200 by file and line with the total, and Stop ends it; forks carry the reads their history shows; a mention counts as read only once admitted
- [x] Follow-ups: project formatter approval covers the package version; `readOnlyMcp` lets the user vouch for servers read-only agents may use; MCP names hashed only on a clash; interleaved thinking on Bedrock and Vertex; Codex `session-id` and residency headers; git guidance in `bash`; `bash` `workdir`; grep on several threads; tool arguments checked against their schema; the read record kept across restarts
- [x] Gaps against opencode: interleaved thinking for API-key Claude with a budget; plan mode reads with bash, delegates to explore and loads skills under a `read_only` guard, primary agents' prompts ride on their turns so a switch keeps the cache; whole-tree captures chained within a step; shell reads count in the read record
- [x] External review at 0a38564: Chat Completions tool messages before same-turn text, `reasoning_content` back within the tool loop, OpenRouter Claude breakpoints on tool turns; built-in formatters only where the project uses them, rustfmt on the edited file alone; MCP tool names in provider-safe characters; refusals visible and context-window stops compacted; unrun calls closed after Stop; compaction under `limit.input`; glob newest of the whole walk; webfetch capped while streaming
- [x] Post-edit checks: `drift.json` `checks` run after edit, write and apply_patch (per file with `$FILE`, else once), problems added to the result, Stop kills them with their process tree

#### Retained features still on the OpenCode database (pending native UI work)

- [x] Removed-workspace purge: `POST /workspaces/{id}/purge` deletes a removed workspace's conversations, shell output and undo history (409 while it is back on the sidebar or one runs); then the shell forgets it, grants and trusted commands included
- [x] Transcript search reads `drift.db` on its own read-only connection (workspaces on the sidebar, spawned threads included)
- [x] Settings > Storage sizes `drift.db` and the engine's folders; "Clean up now" runs the engine's housekeeping; compact is refused while a conversation runs; the event-log rules, analyze and the daily cleanup timer are gone (the engine cleans every six hours)

## M4: cutover

- [x] Every legacy capability in `docs/research/opencode-exit-inventory.md` checked off or explicitly dropped (audit at 016f239cc; the gaps it found follow)
- [x] A stored part that does not parse loads as a raw unknown part instead of failing the conversation's read, kept byte for byte for export (imported history, newer builds)
- [x] Commands carry a skill's `argument-hint` and its subcommands, so the slash menu offers presets again (`config::arguments`; a wrapper that calls one skill inherits them)
- [x] Tests for two carried bounds (and the shell throttle no longer bursts after missed ticks): a socket that lags catches up from the ring or resyncs; a noisy shell command shows progress at most every `SHOW_EVERY`
- [x] `drift-migrate` conversations: shared and channel databases read in one read transaction each, at startup and after a workspace is added; each conversation whole or not at all through the one writer, once (`imported_session`), in its workspace or its repository's; ids re-minted in order; unmapped parts kept raw in an `opencode` envelope; step bookkeeping and unread display copies dropped; archived ones get a week from the import
- [x] Import bounded: read and written in pages (hidden until complete, an interrupted one redone), oversized parts streamed, checkpoints off the shared connection (62 s, 91 MB peak, reads held at most 123 ms on a 19 GB source)
- [x] Imported edits from a conversation's newest 30 messages and the past week get undo records rebuilt from opencode's diffs against today's files; Claude thinking keeps its signature; undo names files it has no record for (`unrecorded`)
- [x] `drift-migrate` settings, once each (`opencodeImported`): `auth.json` keys for providers Drift has and Anthropic/OpenAI/xAI sign-ins into the credential store, never over Drift's own; MCP servers from `opencode.json` and the shell's `mcp_server` table into `mcp_config`, on only when enabled and approved before, `{env:}`/`{file:}` read; global `model`, `instructions`, `permission` into `~/.config/drift/drift.json` when there is none; the rest (tools, plugins, other keys, unrenewable sign-ins) in the report (`opencodeImportReport`)
- [x] Import shown while it runs (sidebar progress); an interrupted conversation is finished by the next run; opencode's `AGENTS.md`, agents, commands and skills copied to `~/.config/drift` (home agents and commands now read from there directly); a workspace's conversation list loads once per connection, not on every switch
- [x] Before cutover: pending V2 `session_input` work checked (only test fixtures here); the import names any conversation holding queued prompts instead of running them
- [x] Import summary shown once in a dialog after a run that brought anything in or left anything out (`opencode_import_summary` hands it out once), left-out items grouped and worded in every locale
- [x] Usage limits read Drift's own credential (`Engine::current_credential`), renewing an expired sign-in through the engine's refresh lock
- [x] xAI SuperGrok sign-in (device code, renewed like the others); opencode's xAI sign-in imported
- [x] Model modes back in the catalog (fast, ultrafast, flex, pro from models.dev `experimental.modes`, as opencode listed them): own entries and prices, the base model on the wire with the mode's body fields and headers
- [x] A mode and its base take each other's signed reasoning (one model on the wire); another model reads earlier finished thoughts as plain text, as opencode sends them
- [x] MCP rows: one on/off switch, connect button always in place (greyed out while off); read-only trust moved into the edit sheet, on for a new server, kept across saves
- [x] LSP diagnostics after edits (`lsp` module; a project's drift.json may only turn a server off) (replaces what upstream's `edit`, `write` and `apply_patch` reported; the plan's "Dropped" and "Post-edit" rows already say so):
  - Language servers from a built-in table of about thirty (opencode's, without its downloads), used only when installed on PATH or in a project's `node_modules/.bin`; `drift.json` can add, replace or disable one, as with formatters
  - Rooted at the nearest project marker (Cargo workspace, lockfile, `go.mod`, `*.csproj`, ...), one per root; started when a matching file is first read, so ready by the first edit; adopted into a process tree, shut down when idle or the engine stops
  - After a writing call (and after formatters), report the touched files' errors within a short wait, bounded in count and size, appended to the call's result and kept in its metadata
  - A server that is missing, crashes or answers late never fails or delays the call beyond the wait; the result just carries no diagnostics
  - No model-facing `lsp` tool for now; revisit once diagnostics prove useful
- [x] Model-family base prompts chosen from the catalog entry (Codex/GPT, Claude, Gemini, default), Drift's own text; the shared worktree, `<system-reminder>` and final-answer rules in each (`shared.txt`, kept under any replacement)
- [x] Delete the shell MCP runtime and Jev routing (with the legacy sidecar manager, the config watcher and `engine_db`; agent overrides now `prompts.rs`, the old approval fingerprint lives in the import, the orchestrator agent is a native built-in) (`mcp.rs`, `mcp_external.rs`, `tool_routing.rs`, their commands, remote gateway entries and watcher hook); the UI no longer calls any of them
- [x] i18n sweep: drop keys the native UI no longer uses from every locale (ten more after the cutover: fork, startup, usage, MCP status and move keys; keys built at runtime kept; earlier: `drift.mcp.description`, `drift.mcp.authenticate`, `drift.settings.prompts.astraDescription`)
- [x] `DRIFT_*` env vars and `drift` data paths (the engine reads only `DRIFT_*` of its own; config in `~/.config/drift`, the app's data in its app data folder, `drift-engined` defaults to `~/.local/share/drift`; opencode names remain only where Drift reads opencode's data or speaks its protocols: the importer, the companion discovery probe, the ChatGPT sign-in originator)
- [x] Delete `engine/*`, `@opencode-ai/sdk`, overlays, `build-engine.ts`, `build-extensions.ts` (with the sidecar binary, the opencode update workflow and its CI steps; sounds moved to `src/assets/audio`, opencode's license to `licenses/`; `bun run dev` runs the native engine only)
- [x] Remote gateway collapses into the engine router (in process; the event socket closes when its device is signed out)
- [x] Docs rewritten for the new engine (README, `docs/engine.md`, architecture, MCP, extensibility, store, remote, CONTRIBUTING; research and comparison docs kept as history)
- [x] Review after cutover:
  - [x] Imported opencode permission rules kept in written order and the whole list reversed (opencode's last match wins, drift.json's first); `"*"` and a single top-level decision come in as kind `*`, anything else is reported; a broken `opencode.json` falls back to `opencode.jsonc`
  - [x] Orchestrator driven by the engine (`session/drive.rs`, `nudge` parts, 30 per user prompt counted from the transcript); the app only shows how a turn ended
  - [x] Compaction tail a quarter of the model's compaction point, 2k to 15k (a 32k local model no longer compacts every step; large windows keep the 15k they had)
  - [x] Summary on the conversation's own model with a warm cache is the turn's next request plus the instructions (`step_request`), read at the cached price; otherwise lean (files by mention, tool results cut to 2,000 characters); one-shots retry provider faults with a turn's backoff, a compaction's shown as `session.retry`
  - [x] The system prompt's date is the local one (`platform::clock`)
  - [x] Tool descriptions match the tools (edit/write one-line result, webfetch's 64 KB bound and saved file, read's image limits, task's missing delegation tools), pinned by a test; webfetch's own 100k cut dropped so the saved file holds the whole page
- [x] Parity gaps after cutover:
  - [x] `glob` and `grep`'s `include` read patterns as ripgrep's `--glob` (`*.ts` at any depth), one matcher; a Stop ends the glob walk
  - [x] Language servers: opencode's list where installed (PATH or `node_modules/.bin`), rooted at the nearest project marker, warmed by a read, pull diagnostics
  - [x] Agents with `mode: all` or no mode are both primary and delegated to; `hidden`, `disable` and `defaultAgent` read; opencode's `default_agent` imported
  - [x] Files of offered skills read without asking (secrets excepted)
  - [x] The system prompt lists only skills and subagents the agent may use
  - [x] Long-prompt pricing from models.dev's context tiers
  - [x] `read` errors on an offset past the end and says when a file is empty
- [x] Perf numbers versus M0 baseline (Baselines in `docs/engine-rewrite.md`; `bench:engine` now runs a native turn): cold start 32 ms vs 1012, prompt to provider 4 ms vs 1066, about 4.6k tokens per turn vs 9.4k

## M5: hook seam

- [ ] `Hook` trait with serde types
- [ ] Prompt overrides as an internal hook
- [ ] Background task controls: move a running foreground task to the background; add a follow-up to a running background task
- [ ] Measure `edit` miss rates per model family before considering any fuzzy fallback
- [ ] MCP resource templates as a tool (`mcp/resources.rs` lists and reads resources only)
- [ ] A plan file the plan agent may write (opencode allows `.opencode/plans/*.md`), so a plan survives compaction
- [ ] Only if the UI or headless use wants them: user-run `!command` turns, `@agent` mention parts, `format: json_schema` structured output, project references
