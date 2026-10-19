# opencode v2 against Drift

Compared on 2026-10-19. Drift's last opencode backend was 1.18.33, vendored commit `7f964bbb00`
(Drift commits `2e0bc6c052` and `24d0000c1a`, deleted at `8033f3efb6`). opencode built v2 on a
separate branch from June, promoted it to `dev` on 2026-10-10 (`5caf1f9f6`) and deleted the v1
engine (`packages/opencode`). The head read here is `501b15af2` (v2.0.32), 4,741 commits after our
base. The v1 line went on to 1.19.3 with nothing the native engine lacks.

What Drift adopts is decided in "After the opencode v2 review" in
[engine-rewrite.md](../engine-rewrite.md) and tracked in `CHECKLIST.md`. This report is the
evidence behind it.

## Reading the source

`examples/opencode` is older than v2's promotion. Clone the head into the scratch directory and read
it there:

```sh
git clone --filter=blob:none https://github.com/anomalyco/opencode.git oc && git -C oc checkout 501b15af2
```

- `specs/v2/session.md` and `specs/v2/tools.md` state the session and tool contracts.
- `services/www/src/docs/content/*.mdx` are the v2 user docs; `migrate-v1.mdx` lists what changed.
- `packages/core/src` is the engine, `packages/ai/src` the provider wires, `packages/codemode` the
  Code Mode interpreter.
- `git log --no-merges --format='%h %s' 7f964bbb00..501b15af2 -- packages/ai` lists the wire fixes.
- Find a PR's commit with `git log --format=%h -1 --grep='(#NNNNN)'`.

## Findings in Drift's code

| # | Drift today | v2 | v2 source |
| --- | --- | --- | --- |
| 1 | The system prompt is rebuilt every step (`session/prompt.rs` `system`): the date, model name, MCP instructions, skills list and instruction files sit inside it, so any change rewrites the first cached block | A frozen baseline per session; a changed source is added once as an instruction update in the conversation; sources ordered most shared first; session id removed from the prompt (#51960) | `packages/core/src/instructions/`, `session/instruction-state.ts`, `session/instructions.ts`, `specs/v2/session.md` "Instructions Are Value Deltas" |
| 2 | A reply that stops at the output limit ends the turn (`session/turn/steps.rs` `StopReason::MaxTokens`) | Adds a continuation note and goes on; fails after two length stops in a row; a cut-off tool call's error carries a 2,048-character input excerpt (#53876) | `a3f7f42ac`: `session/runner/llm.ts`, `publish-llm-event.ts`, `step.ts` |
| 3 | Stream idle limit 300 s (`llm/http.rs` `Timeouts::default`) | 15 minutes, because reasoning models stream nothing for minutes (#55032) | `packages/ai/src/schema/options.ts` `DEFAULT_CHUNK_TIMEOUT_MS` |
| 4 | `Context::resolve` (`tool/context.rs`) joins `~/x` onto the workspace | `~` expanded in tool paths (#45605) | `a60917496` |
| 5 | `tool/schema.rs` reports type problems and refuses the call | Repairs input first, only where the schema leaves one reading (#45002) | `packages/core/src/plugin/tool-input-repair.ts` and its tests |
| 6 | `apply_patch` folds curly quotes, dashes and odd spaces (`tool/patch.rs`); `edit` matches exactly | `edit`: exact, then the same folding, then trailing whitespace per line, offsets mapped back to the file; per-region line endings in mixed files (#39258, #53166) | `packages/core/src/tool/plugin/edit.ts`, `packages/core/test/tool-edit.test.ts` |
| 7 | Claude effort is one top-level `output_config.effort` (`llm/anthropic/request.rs`), so a level change misses the cache | Claude Opus 5+ and 5.1+ keep the top-level effort and add a system message carrying `output_config.effort` at the change (#48513); GPT-6 Sol and Luna likewise on Responses (#51339) | `packages/ai/src/effort-updates.ts`, `protocols/anthropic-messages.ts`, `protocols/openai-responses.ts` |
| 8 | Reminders are `<system-reminder>` text in user turns | Mid-conversation `role: system` messages on Claude Opus 4.8+ and 5+ (#43180, #52568); thinking `block_binding` with `prefix_mismatch_behavior: drop_block` on newer Claude (#47884); `between_tools` thinking (#53601) | `protocols/anthropic-messages.ts` `supportsNativeSystemUpdates`, `applyThinkingBindingDefault` |
| 9 | `cache_key` is the session's own id (`session/turn/request.rs`) | Children use the parent's id, forks the source's, for `prompt_cache_key` and Codex `session-id` (#51931, #50495) | `packages/core/src/session/affinity.ts` |
| 10 | Gemini `MALFORMED_*` finishes map to `StopReason::Other` (`llm/gemini.rs`) | Retried (#56232); about 230 `fix(ai)` commits harden the wires | `packages/ai/src/protocols/gemini.ts` |
| 11 | A turn running at shutdown is not resumed (`engine-rewrite.md`, async workers) | A write-ahead claim per busy period; claimed top-level sessions resume with a continuation note, bounded attempts, stale running calls failed first (#36105, #40989). Users asked for an off switch (issue #56032) | `packages/core/src/session/execution.ts`, `session/execution/restart.ts`, `run-coordinator.ts` |
| 12 | `drift-migrate` reads v1 config keys and v1 tables | v2 keeps `opencode.db` as the file name and adds its own tables (`session_v2`, `session_message`, `session_inbox`, `credential`); config moves to `permissions` (ordered array, `bash` is `shell`, `task` is `subagent`, `write` and `patch` are `edit`), `agents`, `commands`, `mcp.servers` with `disabled`, `providers` | `packages/core/src/database/migration/`, `v1-migration.bun.ts`, `services/www/src/docs/content/migrate-v1.mdx` |

### Wire fixes to sweep

From `fix(ai)`, the ones most likely to apply to Drift's own adapters: null Responses stream fields
(#56669), unknown stream events (#56286), mid-stream connection loss reported as such (#52634),
results for trailing tool calls (#52421), empty unfinished reasoning items dropped on replay
(#53603), Anthropic `request_too_large` as overflow (#45144), error messages read from common body
layouts (#52014), tool schema roots declared as objects (#51153), outbound surrogates sanitized
(#44880), Gemini parallel tool results merged into one turn (#43814), Gemini thought signatures on
visible text (#43984), unique Gemini call ids (#44051), PDFs as file parts on Chat Completions
(#50933), `prompt_cache_key` clamped to 64 characters (#44571).

### Wire sweep results

Branch `wire-sweep`: the 233 `fix(ai)` commits in `7f964bbb00..501b15af2`, plus `fix(provider)` #54164. Each fix
has a test that failed before it.

The 14 named:

| PR | v2 fixed | Drift |
| --- | --- | --- |
| #56669 | null Responses stream fields read as absent | Fixed in 66034bbbe9 (a null call id gets an engine id; null usage, output, reason are absent) and b4162d87f0 (Gemini nulls) |
| #56286 | unknown and untyped stream events | Fixed in 79615c9925: untyped frames ignored, a bare `{"error"}` fails the reply, on Responses and Messages (Bedrock reuses Messages); unknown typed events were already ignored |
| #52634 | mid-stream connection loss reported as such | Fixed in 697ef11506: "connection lost while reading the response: <cause>" instead of reqwest's "error decoding response body" |
| #52421 | results for trailing tool calls | Not affected: `session/convert.rs` `result_blocks` gives every replayed call a result, an unfinished one "interrupted" |
| #53603 | empty unfinished reasoning dropped on replay | Not affected: the Responses request sends reasoning only with `encrypted_content`, and unsigned reasoning of an unfinished reply is never replayed |
| #45144 | Anthropic `request_too_large` as overflow | Fixed in 83d733d621, with Codex's streamed `context_length_exceeded` and more servers' wording |
| #52014 | error messages from common body layouts | Fixed in 8f7f45a160: `llm::error_message` reads `error.message`, `error`, `message`, `Message`, `detail`, `errors[0].message` on every wire |
| #51153 | tool schema roots declared as objects | Fixed in cf07467fb8: `ToolSpec::parameters` adds `type: object` and drops an empty struct's object-or-array `anyOf`, on every wire |
| #44880 | outbound lone surrogates sanitized | Not affected: Rust strings cannot hold a lone surrogate and serde_json refuses one on input |
| #43814 | Gemini parallel results merged into one turn | Not affected: a step's results are one user message (`convert::push` merges adjacent ones); b4162d87f0 also moves non-response parts out of that turn (#43479) |
| #43984 | Gemini thought signatures on visible text | Not affected: a signed text part streams `PartSignature` and replays as `Block::Signed` with `thoughtSignature` |
| #44051 | unique Gemini call ids | Not affected: a missing id was already an engine id; a repeated supplied id (#44454) fixed in b4162d87f0 |
| #50933 | PDFs as file parts on Chat Completions | Not affected: `compat/request.rs` already sends `{"type":"file","file":{...}}` |
| #44571 | `prompt_cache_key` clamped to 64 characters | Not affected: the key is a session id of 28 characters, and only Responses sends it |

Also fixed:

- Responses items followed by `output_index` when the id is missing, and items given whole only at
  `output_item.done` shown once (#45013, #45081, #45330, #46064, #44595, #45854): 66034bbbe9.
- Responses WebSocket: notifications between responses keep the socket (#47316, #44488), the continuation baseline
  uses streamed items when the completion lists none or re-encrypts reasoning (#47291, #48249): b1fd242766.
- Gemini: function-response turns isolated (#43479), `skip_thought_signature_validator` on unsigned calls
  replayed to Gemini 3+ (#41109), prompt `blockReason` as a refusal (#43070), explicit nulls (#44490), repeated
  call ids (#44454), `API_KEY_INVALID` as refused credentials (#51950): b4162d87f0.
- SSE: events without data and `data: : keepalive` dropped (#50954): 50122dd240.
- Chat Completions: xAI's "Incorrect API key provided" (#52112), usage on the choice and cache hits as
  `cached_tokens` / `prompt_cache_hit_tokens` (#44502), call ids replayed to Claude through a gateway (#45056,
  Claude part): 978e135fa5. Text-valued streamed `error` (#52014): 8f7f45a160.
- Overflow wording of Z.ai, DeepInfra, Novita, Together/TGI, Workers AI, DashScope, llama.cpp (#52104, #52132,
  #52134, #52133, #52128, #45834, #37848): 83d733d621.
- Retries: 409 (#39391), streamed faults named only in words (#46038), spent balance or expired plan said in
  words never retried (#52104, #49195): 81e8f5a522.
- Empty Messages turns left out (#46291): 2383fccb72. Vertex ADC under `CLOUDSDK_CONFIG` (#54164): 0bb83e859c.

Not affected, Drift already does it: #56232, #55032 (Gemini malformed finishes, 900 s idle); Anthropic unknown
events, pings, error events, nullable usage, finish across usage deltas, orphan deltas, pending calls, id scrub,
signature requirement, parallel results in one turn, tools kept under `tool_choice: none`, cache breakpoints
(#43767, #44817, #44500, #43761, #46171, #43289, #43136, #43796, #44713, #41948, #38553, #38725, #46289, #45088);
Responses flat and detail-free errors, error frames, encrypted reasoning, top-level instructions, stateless
replay, orphan deltas, pending calls, file inputs (#48376, #49402, #44537, #44863, #45085, #45050, #44587, #44575,
#43575, #44234, #46297, #38374); Chat Completions finish rules, `[DONE]`, truncated or filtered calls, call
identity by index, empty assistants, tool choice without tools, data URLs, nullable fields, reasoning deltas
(#46036, #44743, #44864, #45850, #46040, #46029, #37847, #45046, #48857, #48862, #44499, #40798, #37558, #45864,
#41210, #48863); Gemini text closed before calls, unknown parts, default args, tool finish, `parametersJsonSchema`,
message-less errors (#46286, #44745, #43111, #40546, #51009, #46069); SSE null frames and retry fields (#50793,
#45093); thinking budgets fitted to the output limit in `session/turn/request.rs` (#51157, #51189); transport and
network errors retried (#43807, #41779); bodies shown when no message is found (#51978).

Not applicable, Drift lacks the wire or route: Bedrock Converse and Mantle, Bedrock Nova and Mistral (Drift's
Bedrock route is Claude on InvokeModel); Mistral, Cohere, Kimi/Moonshot, DeepSeek, Alibaba, DigitalOcean, Groq,
Cloudflare, Azure, Z.ai Responses, xAI Responses WebSocket, Gemini over Chat Completions, Google Interactions,
Vertex Interactions and Mistral routes; media, image, speech, transcription and evaluation routes (#51300 to
#51380, #51632, #51930); the rest of the 233.

Not applicable, v2 internals or features outside a wire fix: provider option typing and passthrough, route ids,
metadata keys, capability defaults, service tiers, request labels, compaction triggers, effort updates and native
system updates (plan rows 7 and 8: #52568, #52426, #43180, #47884, #53289, #43498), thinking `display`,
`disable_parallel_tool_use`, `prompt_cache_key` allowlists (#49121, #41131), Responses `phase` and `type:
message` on input (#38777, #49110; OpenAI and Codex accept Drift's items), tool deduplication and history
validation, trace overhead, abort signals.

Left open: Drift sends a `functionCall.id` to every Gemini model; v2 omits it before Gemini 3 (#44454, #50504).
Gemini 2.5 accepts it today; worth a live check if a 2.x refusal shows up.

### Edit against v2's cases

The ten cases of v2's `test/tool-edit.test.ts` that apply to Drift are ported as
`crates/drift-engine/src/tool/edit/cases.rs`. Before the change Drift passed three: refusing a no-op,
empty, missing or ambiguous `old_string`; `replace_all`; and keeping a BOM and CRLF endings. It
failed seven: typography folding, both trailing-whitespace cases, a mixed CRLF and LF file (Drift
rewrote every ending to CRLF when the file held any), endings of unchanged lines inside a match,
edits that change only `\r`, and refusing an edit that leaves the file unchanged. All ten pass
with `tool::edit::matching`. v2's two concurrency cases are not ported: Drift's file reservations
serialise writers above the tool.

## Features

- **Web search.** One `websearch` tool over providers (Exa, Firecrawl, Parallel, Tavily, TinyFish).
  `websearch.provider` is an id, `random` or `false`. The provider sticks to the session; a 429
  puts it in cooldown for its `Retry-After` (60 s default) and random selection moves on.
  Providers register through plugins (`ctx.websearch.transform`). Firecrawl is called through its
  hosted MCP endpoint. `packages/core/src/websearch.ts`, `plugin/websearch/`, `tool/plugin/websearch.ts`.
- **Warming.** Off by default. After a model request, a keep-alive request every 4 minutes for 30
  minutes, using the session's context plus a transient "reply OK" prompt, response discarded,
  skipped for subagents. `plugin/warming.ts`, `services/www/src/docs/content/warming.mdx`.
- **Cache diagnostics.** Logs which part of the request prefix changed when cache reads drop.
  `session/prompt-cache-diagnostics.ts` (#39139).
- **MCP elicitation.** Answered as forms in the client; spec-shaped capability advertised (#35064,
  #53196). `packages/core/src/form.ts`, `mcp/client.ts`.
- **References.** Named outside folders or git repositories in config, attached in the client,
  guidance in the instructions, git references refreshed daily. `packages/core/src/reference.ts`,
  `services/www/src/docs/content/references.mdx`.
- **Worktree and session tools.** `worktree_create`, `worktree_list`, `worktree_remove`,
  `session_move` (takes effect at a safe boundary), `session_rename`; worktree setup scripts
  (#44455). `tool/plugin/opencode.ts`, `packages/core/src/worktree/`.
- **Plan files.** The plan agent may edit only `~/.opencode/plan/*` (#43710), and is told to
  discuss plans in chat and write a file only when asked (#46905). `plugin/plan.ts`.
- **Steer and queue.** Every prompt is an inbox item; `steer` (default) delivers at the next safe
  step boundary, `queue` waits for an idle boundary, steers first, one queued item at a time.
  `session/inbox.ts`, `specs/v2/session.md`.
- **Subagent model choice.** `subagent` takes `model` (`provider/model#variant`), described as
  "never set unless the user asks", checked against available models; a `models` tool lists them
  (#49194). `tool/plugin/subagent.ts`.
- **ACP.** The CLI serves the Agent Client Protocol for editors. `packages/cli/src/acp/`.
- **Autonomous goal.** `/goal` re-evaluates after each turn and continues until a one-shot judge
  says done. `plugin/goal.ts`. Drift's orchestrator covers this.

## Parts that need a decision first

### Code Mode in practice

MCP tools leave the native tool list by default. The model gets one `execute` tool that runs a
JavaScript subset in opencode's own interpreter (about 10,800 lines in `packages/codemode`), calling
tools as `tools.<server>.<tool>(...)`, plus a synchronous `search` when the catalog is too big to
list. Typed signatures go in the instructions and change as instruction updates, so a server
connecting changes no tool definition. Intermediate results stay in the program and only its
return value reaches the context.

The evidence for the gain is thin:

- Anthropic's post on code execution with MCP (2025-11-04) reports 150,000 tokens down to 2,000 for
  one example, an agent connected to thousands of tools reading only the definitions it needs. Its
  second gain is large intermediate data (a 10,000-row sheet filtered to five rows in code). It
  also says the sandbox, limits and monitoring cost real work.
- opencode publishes no measurement. Its issue tracker since Code Mode became the default shows the
  costs: `execute` had no time limit, so a never-settling promise hung a background worker for good
  (#56741); the whole catalog was re-announced on every reconnect or browser attach, 419 times and
  2.27M characters in one 22-day deployment (#55529); a permission ask inside a program showed `*`
  instead of the tool (#55534); policies could not deny `execute` (#56349); models wrote
  `require('fs')` and failed (#56811). Servers that already wrap their tools in their own code mode
  (PostHog, executor.sh) need special cases (`plugin/mcp-codemode-defaults.ts`).
- Anthropic's API now has its own tool search with deferred tool definitions, which gets the
  definition saving without running model-written code, on Claude only.

For a coding agent with a handful of MCP servers, the definition saving is mostly cache-read tokens
and context room; the intermediate-data saving matters for scraping, browser and data work.

### Compaction by the provider

- **OpenAI.** `/responses/compact` takes the conversation and returns it with an encrypted
  `compaction` item standing in for the older part; `context_management` with `compact_threshold`
  on a normal request does the same in the request. The docs say the item "carries forward key
  prior state and reasoning", which a text summary cannot, and must be passed on unedited. It is
  billed as ordinary input and output tokens. No public quality comparison was found.
- **Anthropic.** The `compact_20260112` context edit compacts in the request once input passes a
  trigger (150,000 tokens by default) and returns a `compaction` block with readable summary text
  (null when it failed). Its tokens are reported as a separate `compaction` iteration, not in the
  top-level usage, so a cost tracker reading only the top-level fields undercounts it.
- **How v2 handles a model switch.** It keeps the full transcript. The checkpoint is used only while
  the provider, model and endpoint match; another model gets the original messages back and, when
  they do not fit, compacts them again with opencode's own summary. Nothing breaks, but a switch
  after a native compaction costs a second compaction. Token counts from another provider are not
  trusted for sizing (`hasMeasuredPrompt` in `session/compaction.ts`).
- **Cost.** About the same as a summary: both read the conversation, mostly from cache, and write a
  short result. In-request compaction saves the separate request and its wait. Drift's own summary
  already runs as the turn's next request on a warm cache.

### Todo usage in Drift

Every `todowrite` call in Kyle's `drift.db` on 2026-10-19 (read-only): 10,547 calls in 539
sessions, 4,172 since the native cutover; 87% from GPT models, 12% from Claude; 3.3% of all tool
calls in those sessions.

- Models use it for long work. Turns that used it had a median of 33 other tool calls, against 4
  for turns that did not, and 68% of turns with 20 or more calls used it.
- They plan more than they track: a median of two calls per turn (the plan, then one update), 0.5
  todo calls per 10 other calls, and 1,355 updates that marked two or more items done at once.
- Lists go stale. Of 4,156 turns with a list, 2,085 ended with every item done, 1,391 partly done and
  680 with nothing done. Of the unfinished ones that ended normally, 1,230 had the list picked up
  again within two turns and 510 never did; sampled replies report work finished that the list
  still shows pending.
- Cost: about 3M tokens written and returned, 54% of it the tool echoing the whole list back as
  pretty JSON, which then stays in every later request of the turn.

Kept, with a one-line result. Codex (`update_plan`) and Claude Code (`TodoWrite`) both ship a plan
tool for their own models, and the planning on long turns is the part that works.

## What v2 dropped and why

- **LSP.** Not ported yet, not rejected. The migration guide says v2 keeps the `lsp` config but runs
  no servers; issue #50916 is open with users asking for it back, and the port is tracked as #38528.
  v1 also had sessions slowed by whole-workspace diagnostics stored in tool metadata (#54280), which
  Drift already avoids by bounding them.
- **Asking before outside paths.** #53585 removed the default `external_directory` ask so build and
  general agents stop prompting, and dropped the managed allows that only existed to soften it. The
  PR gives no further reason; v2 points users at references for outside material. `.env` reads
  still ask.
- **Todo tool.** #35989 (July) removed it from v1 and v2, storage included, with no stated reason.
  Open issues show the sidebar list drifting out of sync with work on non-Anthropic models.

## Other v2 areas

- **Media generation.** `packages/ai` gained image (OpenAI, Google, xAI, Z.ai, Black Forest Labs,
  fal, Replicate, Stability), video (queued routes), speech and transcription (ElevenLabs Scribe,
  AssemblyAI, OpenAI) routes. Core uses them for a realtime voice session endpoint (`voice.ts`) and
  voice delegations from the iOS app; the app previews generated media.
- **Remote pairing.** `opencode pair --remote` publishes the local server through OpenTunnel
  (`@opentunnel/client`), a public HTTPS route under a wildcard certificate, after asking once; QR
  codes carry every address; one-time connect links. Drift's remote access is LAN and Tailscale only
  by design (`docs/remote.md`).
- **Console and policies.** The Console is opencode's hosted service: team workspaces, hosted
  models, a provider gateway, usage and budgets, SSO and SCIM, hosted web search. `policies` are
  binary allow or deny statements (`provider.use`, `tool.use`, `integration.use`) that only tighten,
  read from system config locations and macOS managed preferences at the highest precedence, or
  pushed by a Console workspace within a minute.
