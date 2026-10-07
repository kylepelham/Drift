# Capability gating without context creep

Investigated 2026-10-06 against Drift `next/2.0.2`, `4eb75f8023`, Claude Desktop
2.26454.0 and its downloaded Claude Code runtime 2.1.289. This answers the
adoption question: how can broader task capabilities coexist without making
ordinary coding slower, filling every prompt, or advertising fictional tools?

No optional capability or setting was implemented in this pass. The contract
below is proposed Drift behavior, distinguished from observed Claude code.

## Artifact update and evidence

Desktop updated during the research sequence. The earlier 2.19675 installation
path is gone. Previous reports retain their original hashes/offsets as historical
evidence; they must not be applied to this build.

| Artifact | Identity |
| --- | --- |
| Current Desktop | `Claude_2.26454.0.0_x64__pzs8sxrjxfjjc`, archive package `@ant/desktop` 2.26454.0 |
| app.asar | 44,634,501 bytes, SHA-256 `a576cf531f7497d599e951b34c40f2619b3d1ee6f96fde9d5ba1b895153a073b` |
| Archive verification | 437 entries; all 431 packed entries passed their declared SHA-256 checks |
| Downloaded runtime | `Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude/claude-code/2.1.289/e1f0154146bb/claude.exe` |
| Runtime executable | 249,522,848 bytes, SHA-256 `bcc6d9117aec30ad9414490302a25414359c871f5647e32e49b055c92bf84e0b` |
| Runtime source module | SHA-256 `45b5d53c939c416a07ab8a430360cb40e6b53d62cc40478be364f7a93a47822e` |

The runtime module parsed with zero diagnostics and 19,020 function bodies.
It is one module, not the entire executable's source. Desktop modules were
separately parsed with zero diagnostics. References are syntactic, not a fully
resolved cross-module graph. All offsets are zero-based/end-exclusive bytes.

Only installation code and the specifically named downloaded runtime executable
were read. Nearby account/chat/credential/cache contents were not inspected.
No app code or runtime executable was executed, no inference or GUI automation
ran, and no feature flags/entitlements were queried. Packed-member integrity is
not publisher-signature verification. No proprietary source is committed.

Desktop aliases below identify `.vite/build/` members:

| Alias | Member | SHA-256 |
| --- | --- | --- |
| H | `index.chunk-pkvzgHps.js` | `d906c84df6815d3a670bceff3996dc92f792f4dcb4a64d1f223ee234f397c956` |
| C | `index.chunk-Bb2CN-tL.js` | `3ec8c53f388de44a7d3db58d7a63c0828fa4c4fc8d1ed9d148a53b5b95a999ba` |
| T | `index.chunk-BvAiPpoy.js` | `9993533081a10d83e6ec15f42c898262813fd8dc5ba479b29c95b726d4b755b9` |
| B | `index.chunk-Bb3Yyrov.js` | `08aefc688b76e57f0e070471a1e8aad1910c5d3f30be85b40bba6e1e19b00ca2` |
| M | `index.chunk-6oRvGz9p.js` | `62347b32b0e64f51909f5ed92bf2086947d795b6c6c6ee36264f7b4f88caed06` |

## What Claude actually gates

It has several different decisions. A feature name in a prompt is not the
decision authority for any of them.

```text
settings, policy, platform, backend, SDK version, session type
    -> eligible internal servers and per-tool enabled state
    -> static plus session-specific dynamic tool definitions
    -> inline/deferred placement and conditional prompt sections
    -> model request over available/discovered tools and history
    -> call-time policy, current settings, resource identity and state
    -> bounded result/error, cleanup and later request context
```

### 1. Eligibility precedes the callable registry

H `createProxyServers` builds candidate definitions through
`Fx`, then skips disallowed sandbox placements, calls each server's
`isEnabled(sessionContext)`, combines static and `getDynamicTools(context)`
definitions, and filters user-disabled `local:server:tool` entries. Empty
servers are skipped. Session-specific handlers/disposers and an enabled-name
set are registered only for servers that survive.

Examples show what these predicates mean:

- In-app browser eligibility differs between Code and Cowork. Code checks
  session type, remote/VM placement and `launchEnabled`; Cowork checks whether
  the session has a browser pane plus its browser gate. A working Code browser
  is not presumed available in a remote or Cowork context.
- Terminal tools in T depend on app/organization settings, platform/context
  support and whether terminals run on the host. `getDynamicTools` can add the
  command runner without adding it to every session.
- M's simulator definition combines Code-session,
  SSH, platform/backend, feature and `claudeIosSimulatorAccessEnabled` checks.
  A schema existing in a module is not enough to offer it.
- H directory/branch setup tools have session/backend
  predicates and a current gate check inside the handler.

This is a host policy/registry mechanism. The model does not enable a tool by
writing its name or claiming it has permission.

### 2. Placement is separate from eligibility

H `Fx` and `createProxyServers` attach `alwaysLoad`, search
hints and related placement metadata to eligible tools. Simulator tools can
be made inline for matching developer intents. That verifies intent-list
consumption, not how every intent is produced or a hidden optimal classifier.

There is an important trade-off: some browser/Chrome paths are marked always
loaded, and Code browser tools are not uniformly deferred. This saves search
turns for frequently used operations but can increase context. Do not copy every
always-load rule or infer that Claude always chooses the smallest request.

The host builds/imports some candidate modules before checking `isEnabled`.
Therefore disabled prompt sections do not prove zero startup work. Drift can
require stronger lazy-initialization behavior than this observed registry.

### 3. The runtime decides selective discovery and request schemas

The actual 2.1.289 runtime was checked separately from the host SDK:

| Runtime function/range | Observed behavior |
| --- | --- |
| `EIt` | Rejects unsupported model/Vertex/Foundry routes and absent/unavailable ToolSearch. |
| `aqn` | Chooses direct, search or auto-search after eligibility/mode checks. |
| `Fho` | Auto mode compares estimated schema tokens to a context percentage; falls back to description/schema character size. |
| `f5` | Reconstructs discovered names from tool-reference results, surfaced delta attachments and compaction metadata. |
| `AMo` | Tracks added, returned, removed, hidden and revoked tools plus pending/auth/server changes for catalog deltas. |
| `jqt` | Uses available/discovered sets, permission-sensitive declarations and placement to construct the actual request. Relevant selection is. |

The auto fallback percentage is 10 in this build; the character fallback uses
a separate multiplier. These are observed defaults, not a recommended universal
Drift threshold. Mode/endpoint override logic
includes first-party/experimental-beta rules; native references cannot be assumed
to work through every gateway.

This version explicitly mentions Haiku 4.5+ among supported model families and
separately rejects older Vertex serving stacks. The older 2.1.85 report's
unsupported-model/default claims must not be treated as current universal rules.

ToolSearch itself uses the fresh permitted deferred
pool, invalidates search state, reports pending/failed servers and supports a
bounded wait for servers connecting. Searching is local selection plus a model
round trip, not another routing model. It is not permission to load disabled tools.

The request builder is more involved than simply "send every tool previously
used". It handles permission-sensitive declared tools, surfaced names,
recorded-only definitions and deferred placement. This pass establishes those
branches, not every interaction of their gates. Token estimates cached by tool
names/host do not prove schema-version-perfect invalidation. Drift should key
its own catalog/search state by the revisions that actually affect it.

### 4. Instructions follow resolved capabilities and session mode

H `Kk` receives resolved flags and facts such as
`hasComputerUse`, `hasInAppBrowser`, `hasImagine`, `hasHtmlArtifacts`, plugin/
skill settings, host-loop mode, folders, model identity, bridge/child/scheduled
session type and outputs paths. It appends named sections through a helper
that records their character lengths.

Computer-use instructions appear when computer use is present; opted-out
sessions can get a short stub/settings hint instead. Artifact, browser,
scheduling and design sections have their own presence/session-mode checks.
Scheduling is excluded in bridge/dispatch-child/scheduled sessions in the
observed branch. The compiler substitutes actual host/guest folder mappings,
removes empty model-identity placeholders and removes examples referring to
skills absent from the managed set. It does not just paste every feature manual.

Code has its own construction path. C `buildStartSdkOptions`,
installs browser hooks/guidance only when backend launch
tools and the session's browser server are present. Auto-verification is a
separate setting. It carries framebuffer and mobile guidance when those actual
servers exist. These are different surfaces, not one universal Claude prompt.

The compiler's length counters are observability, not a proven hard aggregate
prompt budget. Custom replacements/appends and remote prompt overrides can
still change size or contradict affordances. Do not claim the compiler prevents
every false contextual statement or makes additional context free.

### 5. Revocation, ownership and fresh state remain call-time checks

H proxy handlers consult the current disable-message
resolver before sending an operation and propagate cancellation. Their timeout
wait allows a pending user approval to finish rather than timing it out as a
stuck backend. Session disposers and server generations track lifecycle.

Individual integrations recheck relevant live facts:

- T terminal dispatch checks the current terminal setting
  again. Reuse/stop requires an owned terminal and declines a user-taken one.
- B Chrome approval checks whether the transport was
  disabled while approval waited, before writing a grant. The later site-check
  path binds approvals to actual hosts and denies a moved/
  stale approved-host mismatch.
- H framebuffer code refuses input on dirty/human-owned,
  suspended or disconnected state, checks before/after delivery and reports
  uncertain delivery. Failed click-region comparison refuses the click.
- C gate changes mark stored gated SDK snapshots stale.
  `applyFlagSettings` validates allowlisted session keys,
  saves changes and coordinates pushing them around in-flight startup/query
  work. Not every arbitrary setting is a supported live mutation.

These are specific observed protections, not proof every feature repeats every
gate uniformly. Withholding a schema alone does not revoke a retained handle.

### 6. Compaction carries operational state independently of summary prose

Full compaction and reactive compaction
record sorted discovered names in
`preCompactDiscoveredTools`. `f5` reads that boundary later. Full compaction
clears old read state and restores attachments through the restore path.

Tool catalog deltas also represent removals, pending/auth changes and revoked
entries, rather than assuming every remembered tool still works. Discovery
survival is distinct from permission survival: a recalled name does not approve
the call or guarantee its app/window/server still exists.

## What this does not guarantee

Claude still relies on instructions for selecting sensible tools, distinguishing
untrusted data from policy, telling the truth about verification and not guessing
app state. Its guards prevent certain invalid effects; they cannot guarantee
correct reasoning, a faithful summary or a truthful final answer.

Do not say "Claude cannot hallucinate context". The grounded properties we can
adopt are narrower: only permitted offers run, state references are checked,
results name what really occurred, uncertainty is explicit, and disabled or stale
capabilities cannot silently execute through known controllers.

There is a host-native limit too. A built-in GUI feature toggle can disable its
controller. It cannot prove arbitrary Python, shell scripts or external MCP code
can never manipulate a computer without OS restrictions. Explicit user bans must
not be bypassed through another backend; known tool paths enforce their policy,
and broad host-code execution requires its existing approval boundary. Describe
the toggle's actual scope rather than promising a nonexistent sandbox.

## Proposed Drift adoption contract

Drift already has admitted config/tool snapshots, filtered offers, hard dispatch
refusal for unoffered tools, read-only checks, MCP revocation and generation-
fenced worker delivery. Reuse those services. The gap is composing new optional
capabilities with one consistent offer/context/lifetime contract.

### One effective capability snapshot

Resolve deterministic facts before building an offer: user defaults and session
overrides, restrictive policy, agent scope, provider/model capability, execution
host, configured backend readiness and registry revision. The same result drives
tools, instruction sections, UI availability and diagnostic reasons. No separate
UI/model boolean that says a feature works while dispatch disagrees.

States distinguish disabled, unavailable/missing backend, unsupported model,
policy-denied, available-but-dormant, active and revoked. Show why and the source
of the setting. An enabled feature is not an approved operation, and dormant
is not disconnected. The model may ask for activation; it cannot flip security
or capability settings by a tool-name guess or generated page instruction.

Project config can restrict capabilities; it cannot grant host/UI/secret access
the user did not approve. Workers inherit a constrained owner snapshot, never
amplify it. User-spawned conversations inherit requested preferences, including
optional worktrees, but remain independent conversations with their own authority.

### Small shared primitives, not a builtin per domain

Use three reusable foundations: app/browser observation and controlled actions,
process/job/session lifecycle, and file/artifact/evidence conversion. Browser
verification, notebooks, 3D/CAD, game testing and RE integrate through these and
appropriate MCP/CLI adapters. CI follow-up uses existing fenced input admission.
Domain recipes are small skills or agent instructions, not a new model runtime.

Metadata can be registered cheaply; heavy backends, software probes, downloads,
connections and timers start only after effective eligibility and real use.
Disabled optional capabilities add no tool definitions or full instruction
sections to a normal coding request. A narrow off/unavailable hint is allowed
when it explains the current requested task, not a catalog of every absent feature.

### Controlled loading and context budgets

- Keep existing core coding offers/prompt behavior unchanged with all new
  categories disabled. No paid classifier or extra discovery turn on that path.
- Small frequently used bundles can be direct. Large enabled catalogs use local
  batch discovery and only selected definitions. Search never exposes a denied/
  disabled tool or initializes an unrelated backend merely to find its name.
- Providers lacking native references get explicit next-request schema updates
  at a safe boundary. Do not emulate Claude reference blocks on unsupported wires.
- Pin tool implementations/schema revisions and record placement. Use registry,
  setting, model and permission revisions for cache/discovery invalidation.
- Give instruction sections and schemas an aggregate prepared-request budget,
  with section accounting. Evidence has a separate bounded projection and
  retrieval handles. Do not invent exact token counts from a character heuristic.
- Stable rules/catalog versions form the cached prefix; changing target/status/
  result facts follow it. Do not resend full screenshots, app histories, every
  job log or every domain skill each request.
- Keep source-linked constraints, selected skill versions, owed jobs and relevant
  verification receipts through compaction. Rehydrate live resources from their
  owners, never from a generated summary's asserted IDs.

Discovery is not itself a speed win. Test full cached versus selective cold/warm
paths, including an extra search turn, changed definitions, model switch and
permission revocation. Do not copy every desktop always-load choice.

### Live settings do not mutate authority through cached offers

Normal configuration/instruction changes apply at a safe provider boundary.
Disabling input, revoking permission or disconnecting an owned controller must
invalidate pending/retained known calls immediately through a generation/closing
fence, including a call whose approval resolves afterward. Distinguish removing
availability from stopping an already-running or user-taken process; cleanup
must not kill resources the user now owns. Report interrupted/uncertain effects
honestly and never replay a mutation just because a channel reconnected.

Store discovery/history separately from callable state. When a capability goes
away, keep audit history protocol-valid without advertising its stale handles
as usable. Validate historical-call encoding per provider, including model/agent
switches. A summary or old tool definition must not restore permission.

## Settings surface

Keep controls grouped by capability/lifecycle rather than every tiny operation:

| Control | Scope and intended behavior |
| --- | --- |
| Computer interaction | Global availability plus session choice and approved app/window/site scopes; separate observation from input; backend/model readiness shown |
| Managed processes | Global availability plus session-owned launches, limits, input and stop controls; short Bash remains unchanged |
| Runtime verification | Off/manual/automatic-when-relevant policy, overridden per session/workspace by trusted settings; no browser proof for code that browser cannot exercise |
| Media/evidence processing | Optional installed/configured converters; explicit privacy/remote-upload authority and limits; no silent switch of provider |
| CI/review follow-up | Explicit repository/PR/branch subscription and budget; off unless armed; existing commit/push authority remains |
| Worktrees | Normally off; per-session toggle inherited by spawned sessions; safe ownership/seeding/return rules, no silent move of a running Plan |

These are proposed user-facing groupings, not new API keys or a claim of Claude's
exact settings UI. Advanced per-tool restrictions already fit agent/permission/
MCP config. Availability, automatic continuation and execution permission are
separate controls; "on" does not mean "anything goes".

## Rollout gates before adding capability count

1. Fix the already reproduced freshness and verification/context failures first.
2. Add the offer/settings/context-budget/lifetime contract around existing
   behavior and pass baseline coding tests before a new controller.
3. Add one vertical slice, preferably managed process plus scoped browser
   verification. Keep it controllable and off until its bounds are verified.
4. Add app control and media/integration coverage separately. Each slice must
   pass disable/revocation, scope, cancellation, reconnect and compaction cases.
5. Only enable defaults after paired task trials demonstrate benefit without
   unnecessary inference, context expansion or weaker accepted code.

Required acceptance includes: identical core offers/sections when off; no
feature startup/discovery/download/timer when disabled; hallucinated tool/handle
refusal; related-tool discovery in one batch; no stale approval or Stop revival;
bounded logs/images/schema text; safe model/agent/backend changes; compaction
with revoked or missing resources; no implicit new visible threads; and data
from files/pages/comments/results cannot activate features or approve calls.

Measure startup/idle cost, prepared sections/schema bytes, actual provider
usage/cache behavior, tool/model turns, time to accepted output and repair work.
Fake providers prove offer/control correctness, not higher-quality decisions.
Live coding plus non-coding controls need fixed model/effort, identical inputs,
real acceptance oracles and explicit cost/time budgets. More tools is not a
quality metric, and source inspection does not justify a zero-regression promise.

## Verification and reproduction

Scratch helpers are `claude-desktop-probe.ts` and `claude-runtime-gates.ts` under
`docs/research/private/probes`. The runtime helper reads the named
executable as bytes and indexes the bounded module without evaluating it.
The desktop helper now accepts an explicit archive resource path so an upgrade
cannot silently reuse old offsets. No raw source/index is committed.

Drift's existing unoffered-tool, admitted-config, read-only and MCP-disable/
redefinition fixtures are the immediate reuse checks. Passing them validates
the current protections; the new contract and rollout acceptance remain future
implementation, not fixtures already provided by this RE.

The probes and the commands that run them are kept locally in `docs/research/private/`, which is
not committed.
