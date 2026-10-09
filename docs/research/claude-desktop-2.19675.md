# Claude Desktop 2.19675: mechanisms beyond Drift's existing capabilities

Investigated 2026-10-06 against installed Windows package
`Claude_2.19675.1.0_x64__pzs8sxrjxfjjc` and Drift `next/2.0.2` at
`4eb75f8023`. This is an inspection of the actual desktop package, not another
comparison against the handoff's Claude Code 2.1.85 CLI.

The earlier [agent-quality priorities](claude-agent-quality-2.0.2.md) still
stand. This report adds desktop-specific candidates and excludes things
Drift already handles well. No runtime behavior changed or feature was enabled.

Subsequent user direction: worktrees are wanted as an optional per-session toggle,
normally off, with the choice inherited by spawned sessions, not automatic
isolation for every task. The broader host-native/model-agnostic coverage pass
is `drift-versatility-2.0.2.md`; that records additional process, app and media
boundaries without changing this report's static/runtime evidence limits.

## Scope and method

Installed package root:
`C:\Program Files\WindowsApps\Claude_2.19675.1.0_x64__pzs8sxrjxfjjc`.
The package manifest declares version `2.19675.1.0`; the archive's package.json
declares `@ant/desktop` version `2.19675.1`, entering `.vite/build/index.pre.js`.
`app/version` contains `44.4.3`, the Electron runtime version, not Claude's
agent/model version.

The running app was left alone. Only installation files were read. No chats,
cookies, credentials, account cache, live feature flags or private network
traffic were inspected. No inference, browser automation, native screenshot,
VM start or app-code execution was performed. Installation permissions were
not changed. Raw proprietary source was not copied into Git.

A scratch reader indexes app.asar and reads bounded source windows in place.
All 400 packed members passed their declared SHA-256 integrity checks; there
are 406 archive entries, with six unpacked members. These hashes establish
artifact identity, not publisher-signature validity. Native files were hashed
separately rather than assuming their archive declarations match signed files.

The repository's static TypeScript indexer parsed the selected desktop modules
with zero diagnostics. Function ranges and syntactic callers guided tracing;
they are not a fully resolved cross-module call graph. Offsets below are
zero-based, end-exclusive bytes within the named archive member.

IDA Professional/idalib 9.4 analyzed cowork-svc.exe into a scratch database.
It recovered 14,997 functions and useful Go names. Relevant functions were
decompiled and their branches inspected, rather than treating strings alone
as implementation evidence. The target executable was never executed or patched.

## Artifact identities

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `app/resources/app.asar` | 43,091,042 | `37e9d113862178af8a802d1e20db3bcc081d11056e4d8de5b83b9040f9050f8e` |
| `app/resources/cowork-svc.exe` | 13,602,128 | `67cd865dc4767bf4b1e6b452d61f3f8968571d21f04f1bba7c91cec9675c2c66` |
| `app/resources/app.asar.unpacked/node_modules/@ant/claude-native/claude-native-binding.node` | 3,679,056 | `9f7da0a84f9331f90722fdcd59bcf4172cc6f758627ce0ec26be2d61c9197945` |

Short aliases used below refer to these packed members:

| Alias | Member under `.vite/build/` | SHA-256 |
| --- | --- | --- |
| D | `index.chunk-Bi7IpxcB.js` | `e8b42dfeb988746e598b98d4c0144fdd6e3a655387da50f12f12b65365b64724` |
| B | `index.chunk-gYMpj5vm.js` | `d27079996607ff2f443c65d6d75d16b7a9fefe80d38d05ea98cb3bd66cf7cf9b` |
| W | `index.chunk--kbxP22v.js` | `2e2b73a96e0550981d888ea13f4e369c996f0846aed2f3bc2daef289dc26d3c8` |
| C | `index.chunk-Qn94p2hQ.js` | `f73998661a72e40253f4157ae574b9fd0f49332b38b32ec601d089b844eed104` |
| S | `index.chunk-BaivF2vS.js` | `2d75a0b735954ac21103a4f78c507dc49fff904efdc559f2833599d8f1242fc5` |
| T | `index.chunk-p9zZ4vou.js` | `560c7d8c18ce26b8f214865205f0cf3b162af9dab4f3947a2ed937944ac5f9fb` |

## 1. Session-managed runtime verification, not another static preview

This is the strongest additional quality candidate for web-app tasks.

Desktop's `buildStartSdkOptions` in D checks whether the
backend supports launch tools and the session has the `Claude Browser` server.
It installs PostToolUse hooks and appends verification guidance selected by
`m_`. Guidance depends on auto-verify settings and browser
mode; mere presence of the prompt does not prove it is enabled for this account.

The workflow around D is specific:

- Verify only changes observable through the preview's rendered page, served
  API or logs. Skip tests/types/tooling and other runtimes the preview cannot
  exercise. This avoids starting a useless server for every task.
- Use a managed server from `.claude/launch.json`, reused when already running.
- Prefer console/server/network text for errors, accessibility snapshots for
  content and stable element references, and computed CSS for exact styles.
- Exercise click/fill behavior and confirm the result; resize only where useful.
- Fix actual failures in source and repeat relevant checks. Use screenshots
  for appropriate visual proof, not to infer exact CSS values.

This is backed by tools, not just prose. B defines `preview_start`, logs,
snapshot, inspect, click/fill, screenshot and other browser operations.
`Q`/`de` dispatch through policy and bounded
operations. The implementation checks session/worktree ownership, rejects
stale IDs, and rechecks origin after approval. B has 30-second
operation bounds, a separate 45-second JavaScript bound and actionable timeout
responses for navigation, hidden/minimized views and unresolved promises.
The list path includes recently ended servers and their retained logs.

PostToolUse `O_` in D limits repeated reminders, carries a
plan's verification section when present, and avoids falsely claiming a file
was shown when preview opening failed. Its source distinguishes another
conversation's server from a server this session can actually reach.

Drift already has file/PDF/image previews, safe static HTML, post-edit checks,
LSP and generic browser-capable MCP integration. Those are not missing features.
Its HTML viewer deliberately removes scripts and controls and enforces a
script-denying CSP (`src/ui/html-preview.tsx:23`). It is not a live dev-server
browser that the agent owns and can test. The Bash contract also disallows
long-lived servers (`crates/drift-engine/src/tool/prompts/bash.txt:6`).

Remaining candidate: a session-owned dev-server/browser verification lifecycle,
preferably reusing an existing browser MCP rather than duplicating automation.
Keep the static viewer's safety policy. Browser content remains untrusted and
must not obtain engine credentials or privileged desktop access. DOM, console
and network proof should work without native screen-capture scripts. Preserve
the no-conversation-tabs rule; Desktop's browser-tab layout is not a requirement.

Acceptance: catch an interaction/runtime failure that a green build missed,
return proof of the changed behavior, reuse the correct server, and leave an
unrelated session's server untouched. Test closed pages, redirects during
approval, unavailable servers, Stop and remote use. Measure extra model calls
and verification time; this need not run on non-previewable work.

## 2. Opt-in task worktrees with lazy setup and safe ownership

The material difference is separate working copies, not more workers.

W `createWorktree` and `createWorktreeInScope` create a
real Git branch and linked checkout. The path validates repository backlinks,
trusted anchors and ref-write paths rather than trusting a supplied.git
redirect. The session owns a lease. Post-create failure aborts the setup and
attempts to remove its partial worktree.

W uses `git worktree add --no-checkout`, then either
staged/deferred checkout or full checkout. Origin refresh and other setup work
can overlap. `stageCheckout` and `fullCheckout` are separate.
Dependency seeding waits on readiness,
checks the session's lease and observes cancellation. It is not evidence that
every dependency can be reused safely or that all setups are fast.

D's `createLazyWorktreeHooks` supplies a worktree through
WorktreeCreate when the lazy mode is selected. WorktreeRemove refuses a path
the app did not lease or can no longer release. Other source paths explicitly
warn when the underlying CLI is too old for first-edit worktree creation.
Static presence does not establish the default choice on this installation.

Drift already serializes physical-file writers and whole-workspace checks,
handles undo across overlapping workspaces, and runs background workers.
Those guarantees should stay. A fork or `/spawn` copies conversation history
but keeps its workspace (`crates/drift-engine/src/session/branch.rs:45`,
`crates/drift-engine/src/session/tree.rs:39`); it does not create a separate
branch/filesystem. Locks therefore prevent simultaneous engine writes, but
do not make different tasks' code, imports, builds or tests independent.

Remaining candidate: user-selected isolated task checkouts for genuinely
independent changes. Preserve permission/config/read evidence and snapshot
ownership, pin the base revision and define an explicit conflict-aware return
of changes. Do not silently move a running admitted Plan to a different folder.
Lazy setup needs a safe transition boundary, not an exception to move guards.
It must not auto-create visible conversations or replace the native engine.

Acceptance: two conflicting implementations/builds cannot contaminate each
other's working tree; the user's dirty base remains intact; cancellation leaves
no half-owned checkout; applying work back detects newer base edits. Measure
setup and dependency cost against fewer conflicts and parallel completion time.
Read-only exploration should not pay for an unnecessary checkout.

## Native Cowork isolation is real, but not the same feature

IDA established that this package also has a separate native VM boundary:

| Native function VA | What was inspected |
| --- | --- |
| `WindowsVMManager.StartVM` | Builds configuration and calls `CreateComputeSystem`; failure cleans startup resources. |
| `VMConfig.BuildHCSDocument` | Constructs HCS device maps, including virtual disks, Plan9, HvSocket and ReadOnly settings. |
| `WindowsVMManager.AddPlan9Share` | Tracks share name/path/port/read-only settings under a lock, refuses changes in a guarded manager state, and handles duplicate definitions. This does not imply all shares are read-only. |
| `RPCServer.Start` | Creates a Hyper-V socket listener using the supplied VM identity on port 51234. |
| `HVSocketListener.Accept` | When peer binding is enabled, compares the accepted VM GUID to the bound identity; closes and rejects a mismatch. The unbound branch is distinct. |

This supports a native isolation implementation, not merely a model instruction
to stay in one folder. It does not prove every Code session uses a VM, all mounts
or network access are restricted, or that a complete sandbox security audit passed.
The service also has conditional signature-verification initialization; its
presence alone is not proof that verification is always enforced in every mode.

Drift's approvals and process-tree cleanup are useful, but are not an OS sandbox.
Do not copy Cowork's Windows service/VM architecture into Drift just for parity.
It has substantial platform, startup and dependency costs and conflicts with a
blind sidecar adoption. Working-copy isolation is a smaller first experiment;
optional OS sandboxing needs its own product decision and threat model.

## 3. Opt-in PR follow-up driven by changes, not model polling

C implements an AutoFixEngine, not just a UI toggle. `start`
installs a 60-second sweep plus session/turn/drop events. Background throttling
and GitHub reads occur in host code. This is polling at the application layer,
not a webhook-only system and not a model repeatedly calling `gh`.

`sweep` selects enabled eligible sessions, chooses an owner
for each PR and cleans detached targets. `checkSession` reads
checks/reviews/comments, keys failures by head SHA, avoids duplicate notifications,
settles mergeability, and coalesces parked work. It includes bounded reminders,
send-failure handling and rechecks that its state still belongs to this session.
Near delivered wakes are recorded separately from sending;
dropped unsent work can release its comment IDs for a later attempt.

There are real input-trust controls. Review/comment triggers filter bots or
owners/members/collaborators and exclude the current user's own comments.
Quoted GitHub text is bounded and sanitized, distinct from app-origin CI state
near C. The desktop's own auto-fix prompt treats enabling the
feature as authorization to fix and push that PR branch. That policy is not
automatically appropriate for Drift.

Drift already starts serialized follow-ups for worker completions and async
answers, with Stop fencing and durable admission. What is absent is a
commit-scoped external CI/review subscription. A user can ask Drift to run `gh`
today; that is not the same as retaining an opt-in monitor after a turn ends.

Remaining candidate: optional follow-up for one approved repository/PR/branch,
using the existing engine admission and cancellation model. Treat comments
as untrusted data. Keep Drift's explicit commit/push rules and never infer
authorization from comment text or a forged event-shaped tool result. A new
commit, user Stop, archive, sign-out or revoked subscription must invalidate
stale wake-ups. Do not build a second autonomous agent runtime for this.

Acceptance: a new failure prompts useful work once; a stale/partial CI response
cannot report success; changed heads supersede old failures; Stop cannot be
undone by a timer; untrusted comments cannot authorize unrelated actions.
Measure human wait and repair time, not just first-token latency.

## 4. Fast side replies while the main task is busy

This is a response/UX candidate, not established faster code generation.

D `Ef` supplies narrow aside instructions: one to three
sentences answering the new message from available context, acknowledge a
correction as queued, and never claim it already happened. It distinguishes
waiting for the next step from the immediate Stop button.

The manager's `start` performs a separate side query,
tracks prompt UUIDs, skips duplicate in-flight asks, handles synthetic/empty
responses, times out at 45 seconds and records cancellation. `consumed`,
withdrawal and teardown cancel stale side work. The original message still
reaches the working agent. A plugin note tells that agent what was already
said, so it can avoid repeating or contradicting an aside.

`zf` requires an `askSideQuestion` method. `Yf`
additionally checks plugin/remote readiness, arming and a
feature gate. S `askSideQuestion` sends a `side_question`
SDK control request. D has a current-turn history shim for older CLI snapshots,
bounded to 12,000 characters. A compatibility threshold is `2.1.280`; the older
2.1.85 CLI investigation is not proof of this newer desktop behavior.

The app-side caller is established, but the CLI/server's side-inference model,
full execution policy and actual latency were not verified here. In particular,
the unrelated Haiku output-style drafter in D is not evidence that asides use
Haiku. No tool-free guarantee is inferred solely from the aside prompt.

Drift already acknowledges admission and steers follow-ups at safe boundaries
(`src/engine/actions.ts:668`). It has attributed async questions too. Those
remain covered. It does not have a separate response path answering a side
question while a long command/provider operation continues.

Remaining candidate: responsive handling of progress questions and corrections
without interrupting useful work. Start with clear engine-owned accepted/queued
status; evaluate a separate, strictly read-only answer only if actual interaction
latency warrants extra inference. It must not execute tools, approve requests,
make unsupported completion claims or race the main answer. Respect model/cost
preferences and cancel it once the main task consumes the message.

Acceptance: a question during a slow operation receives a timely truthful
answer; a correction is delivered once; aside text cannot be mistaken for an
executed change; Stop ends both paths. Compare perceived responsiveness, stale
answers, duplicated replies and total inference spend.

## Examined but not recommended as missing capabilities

| Desktop mechanism | Why it is not a new Drift recommendation |
| --- | --- |
| Task lists and structured `ccd_turn` wrap-up | Drift already has todos, async choice cards and concise final-answer rules. T validates recap/actions and availability; it does not prove tests passed or enforce a universal correctness judge. A compulsory extra wrap-up tool is not justified solely by this RE. |
| Foreground/background workers, progress, cancellation and reconnect | Already implemented and tested in Drift. No need to reopen M3. |
| Basic tool concurrency, streaming, cache usage, output spooling, model variants and MCP | Already implemented. String presence or a new desktop wrapper is not a material gap. |
| Static file previews and file/editor links | Already present. The browser candidate concerns live behavior and session lifecycle, not making another HTML/PDF viewer. |
| Git diff loading and watchers | Desktop has lazy patch-loading and cache invalidation. That does not establish Drift needs another diff service without a measured problem or consumer. |
| Native VM/service architecture | Verified native components, but a wholesale VM/sidecar rewrite is not a recommended speed improvement or compatible shortcut. |
| `ultrareview_launch` | D checks an active query/SDK capability, calls `launchUltrareview` and reports a `remote_agent` task. The review algorithm is outside this wrapper; no superior reviewer or quality gain was established. Drift already supports requested review through skills/subagents. |
| Arbitrary model-spawned visible sessions | Desktop offers gated session proposals. Drift's user-owned branching and no-tabs rules stay; this research does not authorize automatic sidebar conversations. |

## Ranking and evidence limits

The expected benefits are engineering judgments, not measured probabilities.
All four are additional candidates after the earlier proven correctness fixes.

| Order | Candidate | Likely benefit | Main cost/risk |
| --- | --- | --- | --- |
| 1 | Session-managed runtime verification | High code-quality benefit on web-app/runtime-visible changes; less manual checking | Verification calls and browser/server lifecycle; useless on the wrong runtime |
| 2 | Opt-in isolated task worktrees | High correctness benefit for independent concurrent changes; potential parallel speed | Checkout/dependency cost, base conflicts and ownership complexity |
| 3 | Commit-scoped PR follow-up | High workflow benefit for users who want CI/review repair after a turn ends | Untrusted inputs, branch authorization, stale events and repeated wake-ups |
| 4 | Narrow busy-time side replies | High potential responsiveness benefit; code-quality/completion-speed benefit unproven | Extra inference, stale answers and duplicated/conflicting responses |

Browser policy, worktree creation, aside management, PR monitoring and native
VM branches were traced in this package. They were not exercised in a logged-in
Desktop session. Gates, organization policy, platform and the selected backend
can disable them. Normal Chat inference and remote review internals are not
recoverable from these client wrappers. No model weights, training recipes or
cloud-side quality advantage were established.

## Reproduction

Scratch helpers are under `docs/research/private/probes`:
`claude-desktop-probe.ts` and `claude-desktop-native.py`. IDA's database lives
only in `claude-desktop-ida/`. No generated database or proprietary source is
committed. The reader's ion-directory scan is blocked by the protected package;
file tools can list/read individual assets. The conclusions above use readable
indexed archive members and native analysis, not an assumed full frontend dump.

Static inspector regressions and repository gates validate the shared tooling
and documentation tree separately. They do not turn proposed Desktop-feature
adoption into tested implementation or measured task-quality gains.

The probes and the commands that run them are kept locally in `docs/research/private/`, which is
not committed.
