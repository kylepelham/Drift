# A host-native, model-agnostic task agent

Investigated 2026-10-06 against Drift `next/2.0.2`, `4eb75f8023`, and the installed
Claude Desktop 2.19675.1 package. This extends the desktop RE to task coverage:
what prevents Drift from doing native-app work, 3D assets, games, training,
media analysis and interactive reverse engineering?

## User decisions

- Direct host execution remains normal. Do not put the agent in a mandatory
  VM/container or make Claude's private runtime a dependency.
- Worktrees should be supported but optional, toggled per session, with the
  choice inherited by spawned sessions. Ordinary shared-workspace work stays
  the default. Do not automatically isolate every worker or create sidebar chats.
- Live runtime verification and opt-in PR follow-up are desirable directions.
- The target is broader than a coding-only assistant and must work through
  portable tools with the user's chosen model/provider.

These are recorded product directions, not implemented features. Worktree
inheritance means inheriting the preference; seeding a child's checkout from
committed or dirty parent state needs a decision before implementation. A
child's preference should remain independently changeable at a safe boundary.

Host-native does not mean removing permissions, exposing credentials to generated
pages, bypassing OS protection, or letting an agent mistake untrusted content
for authority. Generated interactive pages remain isolated from privileged app
APIs even when the agent itself runs on the host.

## Evidence boundary

The installed desktop archive is unchanged:
SHA-256 `37e9d113862178af8a802d1e20db3bcc081d11056e4d8de5b83b9040f9050f8e`.
All 400 packed entries again passed integrity checks. Installation paths,
native IDA evidence and method limits are in `claude-desktop-2.19675.md`.

This pass inspected bounded desktop source and parsed function/caller ranges.
It did not operate native applications, capture the screen, connect to a real
VNC target, use private account data or make paid inference requests. Platform,
backend and organization gates were not queried. A feature compiled into the
Windows package is not proof it is available on Windows or for this account.

An isolated real Drift headless engine, empty fixture home, fake Anthropic
and no-op MCP server established three current limits:

| Probe | Observed result |
| --- | --- |
| A Node command waiting on stdin | `STDIN_CLOSED`, not an interactive prompt; no timeout. |
| A valid tiny PCM WAV submitted as `audio/wav` | Admission returned 400, `cannot be sent to a model yet`; zero provider calls. |
| A video container header submitted as `video/mp4` | Same kind-based rejection before provider/decode. This was not a full video-decoding test. |
| A no-op MCP tool marked mutating, changing no files | Entered filesystem history with `at`, `changes`, `owner`, `server`; the corresponding read-only tool did not. |

These prove runtime/protocol boundaries, not a live model's inability to solve
the task. No model-quality percentage or measured speed gain is claimed.

## Current task coverage

| Task | Already possible with Drift | Material missing or awkward part |
| --- | --- | --- |
| Static reverse engineering | Demonstrated IDA/idalib and binary/source inspection through authorized host commands; arbitrary appropriate MCP tools | Warm tool sessions, interactive debugger I/O and useful structured evidence rather than repeating startup/analysis |
| 3D model/asset creation | Write scripts, invoke installed Blender/CAD CLI or MCP, export files and inspect rendered PNGs | GUI control where no suitable API exists; reusable app sessions; scene-aware inspection/verification and output handling |
| Game creation | Edit source, build, test, generate assets and use external integrations | Managed playtest/runtime observation, native-app interaction and persistent processes; a green build is not a tested game |
| ML training/data work | Write code, run finite commands with configurable timeout, use background subagents and external tools | Reusable kernels/terminals, checkpoint/control-oriented jobs and native media intake; a worker is not a persistent process handle |
| Native desktop-app work | A configured app MCP or appropriate CLI already works | No built-in generic host GUI observation/action contract when neither exists |
| Audio/video analysis | Existing playback previews, dictation, manual authorized conversion through installed tools | Clips cannot enter the native model request as audio/video; no common file-to-transcript/frame evidence workflow |
| Interactive dashboards/prototypes | Create files and run an app through available tools; safe static HTML preview exists | Session-owned live execution/interaction and stable artifact lifecycle, distinct from passive file viewing |
| Remote compute | Remote UI control of this host, authorized SSH commands and remote MCP endpoints | A unified remote execution/filesystem/process target; do not confuse a remote client with executing on its machine |

Installed software, licenses, permissions and the selected model's skill still
matter. Adding a tool cannot give a text-only model reliable visual perception
or make a weaker model reason like a stronger one.

## 1. General computer-use tools with fresh observation

The largest coverage gap is manipulating an app for which no CLI/API is
available. It matters to CAD, game editors, native debugging and cross-app work.

Desktop's prompt builder `JO` in `.vite/build/index.chunk-p9zZ4vou.js`,
has a conditional computer-use section. It chooses dedicated
app MCP first, DOM-aware browser second for web tasks, and computer use for
native apps/cross-app work. It distinguishes tool availability from errors:
do not silently retry a failed authorized API through a different, slower or
broader authority. It requests app-specific access and prefers fresh observations
over guessing the user's app state. Tool discovery is batched, not one call
per tool. The surrounding prompt explicitly distinguishes guest and host files.

The framebuffer implementation is stronger evidence than those instructions:

- `Vr` checks attachment/connectivity, human ownership and
  dirty state before/after input and reports uncertain delivery honestly.
- `Or` compares the click region and fails closed on comparison
  failure; click/drag paths use it.
- `Gr` maps the documented display coordinate frame into
  source dimensions. Zoom is a crop of the last observation, not a new frame.
- Tool contracts expose list/attach/screenshot/zoom and
  input operations through ordinary schemas. Sources are configured VNC-like
  screens, which may be VMs, emulators or other sources; a VM is not intrinsic
  to the observe/action contract.

Drift already accepts image/PDF tool results over portable wires and can run
external computer-use MCP tools. What is missing natively is a coherent
host-app/window identity, observation/action generation, focus/user-takeover
policy and observation-to-coordinate mapping. This pass does not prove that
Desktop's native host capture implementation works on every platform.

For Drift, prefer semantic app APIs and accessibility references, then pixels
where necessary. Host capture/input should be an optional trusted integration,
not scripts that bypass this machine's screen-capture restrictions. No native
screen capture was tested here. Preserve approval scopes and an immediate Stop.
Do not give read-only agents permission to control apps.

An ordinary JSON tool result can carry text/accessibility state plus an image,
so Anthropic's proprietary computer-use wire is not a required foundation.
Vision remains a real capability requirement for pixel-only work. Existing image
resizing must be reflected in the backend's coordinate contract; do not leave
the model guessing at crop offsets or scale. Old observations must not permit
blind actions after a window/focus/user change or uncertain prior delivery.

For games and fast simulations, batch predictable actions or use an approved
deterministic controller. A multi-second model round trip is not a 60 Hz control
loop, and changing the prompt cannot fix that limit.

## 2. Persistent processes, terminals and controllable jobs

This is the broadest infrastructure improvement for training, REPLs, debuggers,
rendering, simulators and development servers.

Desktop exposes actual agent terminal tools, not merely a user terminal widget.
`.vite/build/index.chunk-B5IfNQ0X.js` defines run/read/list/open/stop contracts;
`me` registers dynamic tool availability and dispatch.
`se` and `ce` implement run and stop paths.
Commands can outlive a turn, terminals can be reused, and retained output can
be paged/searched. Reuse/stop checks refuse a terminal the user typed into or
one the session did not own. The tool validates that a reusable target is an
idle shell, not an arbitrary program waiting for input.

`.vite/build/pty-host/ptyHostWorker.js` contains spawn/write/resize/kill/shutdown
handling, bounded replay, scrollback and process cleanup. Its SHA-256 is
`cd751f710f32f39a12c533a244a65717d5238698536d1d913ae9edf936fc5172`.
Raw PTY write exists, but do not infer that every agent tool can safely type
arbitrary debugger/REPL input from a low-level worker method alone.

Drift's Bash uses `Stdio::null` for stdin and owns its process tree until the
call ends (`crates/drift-engine/src/tool/bash.rs:176`). Its timeout can be raised
to 24 hours and background subagents can wait on long work, so training and
rendering are not categorically impossible. But there is no reusable process
ID/terminal contract for later input, state, logs, interrupt or checkpoints.
Starting another agent for every process is not necessary to gain that contract.

Keep short Bash calls as they are. Add an engine-owned process/job lifecycle
only where needed: start, observe/read output, approved input, interrupt/stop,
ownership and terminal disposition. Separate user-owned processes from agent-
owned ones. Natural end of a model turn need not kill an explicitly persistent
job; Stop and explicit retention choices must remain clear. A dropped local
future does not prove a remote training/render job stopped.

Use the existing store, cancellation/generation and result-delivery principles,
not a new model runtime. Reconnect restores views without rerunning jobs. Restart
must reconcile or report interrupted/unknown work rather than silently retrain
or replay a debugger mutation. Process controls can live in a session dock/list;
this does not require conversation tabs.

## 3. File-to-evidence pipelines and runnable output lifecycle

The model needs usable evidence, not every possible binary format in its prompt.

Drift's neutral `Block` supports text, image, PDF and stored image/PDF content,
not audio/video (`crates/drift-engine/src/llm/mod.rs:143`). Admission refuses the
other kinds (`crates/drift-engine/src/session/attach.rs:55`). Existing dictation
is a different workflow from transcribing a user's recording, and existing
audio/video playback is a viewer, not model perception.

A portable first step is explicit authorized conversion: timestamped transcript
for audio, selected frames plus transcript for video, scene/object metadata and
rendered views for 3D/CAD, and structured cell/source/output extraction for
notebooks. Prefer installed or configured tools and limits to unconditional
new dependencies. Keep source identity, timestamps/units, crop/frame references
and truncation honest. Do not silently upload private media to a different
provider. Native media encoding is an adapter capability, not a model-ID guess.

Cell-aware notebook operations and reusable kernels can avoid loading megabytes
of stored image output or clobbering IDs/metadata. Scripts and notebook JSON can
already be manipulated today; no native cell/kernel workflow was established
for Drift. Desktop SDK mentions of NotebookEdit do not prove an entire live
notebook execution system in this package.

Desktop's artifact handlers in `.vite/build/index.chunk-DPL2Vb2E.js`, around
implement create/update/list from staged HTML, session ownership
and read-only shared copies, with declared connector use. The remote variant
in `index.chunk-CwgiyKxT.js` bounds staged reads and
rechecks account/artifact state before writing. These are real client lifecycle
mechanisms, not proof of a private image/3D generation algorithm.

Drift can already write/export/render assets with host commands or MCP. The
remaining output problem is making deliverables usable: named/versioned file
references, provenance, inspection proof and, where appropriate, managed live
execution. Preserve the safe static preview; do not run generated JS with
engine credentials. Image/music/video generation can be optional configured
tools. A chat model that accepts images is not necessarily an image generator.

## 4. App integration coverage and honest MCP compatibility

MCP is already a useful portable extension point for Blender, CAD, game editors,
IDA/Ghidra, data tools and other apps. Do not add a hard-coded builtin for every
application or make its commands vendor-specific. Installed/connected capability
descriptions, versions and narrow workflow skills should tell the model what it
can actually use and how to verify the result. Do not claim software is missing
or an app cannot perform an action without relevant discovery/evidence.

One real compatibility boundary remains: Drift advertises roots but no sampling
or elicitation (`crates/drift-engine/src/mcp/mod.rs:266`). A server that requires
interactive client input can fail even though ordinary tool calls work. The
existing `a_server_asking_for_input_is_declined_and_the_call_fails_rather_than_hangs`
fixture establishes the explicit decline behavior. The current question cards
are not automatically an MCP callback implementation.

The installed Desktop's SDK contains sampling/elicitation handler machinery,
but schema/library code alone is not evidence every Desktop path registers or
enables it. Reevaluate callbacks against the integrations users actually need.
Forms should use attributed user responses and proper cancellation; secret/OAuth
input must not become ordinary model-visible chat. Sampling needs explicit
provider/cost/context authority and limits, not hidden arbitrary model calls.
Do not enable it just to check a parity box.

Remote SSH/WSL/compute targets are another optional workflow distinction.
Drift's remote UI already controls this host and remote MCP can expose tools;
a consistent remote filesystem/process target is not identical. Training on a
remote GPU or building against another OS needs truthful host/path/tool facts,
explicit target identity and reconciled outputs, not pretending all paths are
on the desktop. No remote credentials or accounts were inspected here.

## 5. Broader capabilities need better effect and resource contracts

Drift currently connects mutation classification to filesystem capture.
`McpTool::mutates` follows readOnlyHint (`crates/drift-engine/src/mcp/tool.rs:164`),
and the default unknown `touches` compares the whole workspace
(`crates/drift-engine/src/tool/mod.rs:453`). The no-op mutator fixture entered
that history path while its read-only peer did not.

For a GUI click, remote job launch or other non-file effect, a whole-repository
scan can be pointless, and undo cannot reverse every app/network operation.
This pass proves classification, not its timing cost in a large game project.
Benchmark that cost before claiming a speed gain.

Separate effects where a trusted implementation can: filesystem mutation,
application/window state, terminal/process state, network mutation and compute
jobs. Keep permissions and read-only restrictions even when filesystem capture
is inappropriate. Unknown external tools stay conservative; do not mark a
dangerous action read-only merely to make it faster. Resource ownership matters:
two agents must not drive the same window, terminal or scene simultaneously,
while independent read/query jobs may proceed safely.

Return effect-specific receipts and observation identities so the model knows
what happened, what is uncertain and what can be undone. This also gives checks
and artifact handling a better boundary than guessing changed files from text.

## Prompt/process changes worth carrying forward

Reuse Drift's existing family prompts, agent config and skills. Add small
capability-specific instructions only when corresponding tools exist:

- Prefer a suitable app API/MCP, then semantic browser/accessibility operations,
  then pixels. Do not use a slower/broader backend to conceal a tool error.
- Discover related tool sets together; retain reusable tool/app/process sessions.
- Observe before asserting state, and verify the actual deliverable: a rendered
  asset, running game path, checkpoint/metrics or debugger finding, not merely
  a successful command exit.
- Make paths, software versions, execution host, units and output locations
  explicit. Host-native removes guest/host confusion, but not remote-host confusion.
- Match the amount of verification to the task, preserve user constraints and
  carry evidence through compaction. The earlier freshness/verification fixes
  remain priorities; broader tools do not fix them by themselves.

Do not paste Claude's private tool names, model identity or guest paths into
every prompt. Do not make everything a separate agent or require high reasoning
for a simple app query. Ordinary JSON schemas plus text/images can serve multiple
providers; perception and reasoning ability still vary and must be disclosed.

## Ranked coverage priorities

These are recommendations, not implemented capabilities or measured probabilities.

| Rank | Work | Main benefit |
| --- | --- | --- |
| 1 | General app observation/control, using APIs/accessibility before pixels | Opens tasks otherwise requiring a human GUI operator; native apps, CAD/game editors and cross-app work |
| 2 | Engine-owned persistent process/job/terminal controls | Makes training, REPL/debugger work, rendering and long services controllable without wasting model turns |
| 3 | Managed runtime verification and useful output/artifact references | Proves games/apps/assets actually work, with less manual checking; builds on the accepted browser direction |
| 4 | Audio/video/scene/notebook evidence conversion and optional kernels | Makes unsupported raw files usable and reduces irrelevant context; native media remains capability-dependent |
| 5 | Targeted integration/callback coverage and optional remote execution targets | Removes specific app/server/compute incompatibilities without a builtin per application |
| 6 | Trusted effect/resource classification and task-specific prompt sections | Potential speed/quality gain once broader tools exist; avoids expensive irrelevant snapshots and competing UI actions |

Worktrees are supported direction, not a compulsory step or the central answer
to versatility. The native multi-provider engine and host execution are already
the right foundation. Broad capability tests should use a 3D export/render,
game interaction, training checkpoint, interactive debugger, media transcript
and missing/denied tool case, across appropriate models. Measure useful task
completion, total inference, setup/repair time and unauthorized effects, not
just tool count or polished final prose.

## Reproduction

Private probe:
`docs/research/private/probes/drift-versatility-probe.ts`.
It imports the existing conformance harness, uses temporary home/data/workspace
and fake services, and removes them on completion. No real GUI or media contents
were used. It confirmed stdin, media-admission and history-classification limits.
Desktop source windows used `claude-desktop-probe.ts` from the previous pass.

No runtime source, provider API, tool registry or generated client changed. The
worktree preference and host-native/model-agnostic direction were added to the
canonical plan/checklist; the broad capabilities above remain future work.

The probes and the commands that run them are kept locally in `docs/research/private/`, which is
not committed.
