# M2 progress checkpoint

Checked 2026-09-29. Inspection began at `ba3cd9363`; formatter, provider-page and
milestone commits landed during the check, ending at
`b7cdf43e49e4128b41360fd61fa7cce66662dae7`. This review is scoped to implemented
M2 behavior, not M3/M4 architecture. No delegated agents or runtime edits.

## M2 follow-up: blockers closed at db12943b

Verified `db12943b66065d503392d9fb702891e137974014` after fix commit `d1ace08fd`.
Both issues in the historical findings below now have code changes, passing
regression tests and successful reruns of the same black-box probes:

- Plan request omitted write, and the returned write was refused without creating
  a file. Dispatch checks the run's offered tool set before permission/snapshot.
- Replacing a delayed MCP connection's config left the new definition awaiting
  approval with no old tools, and a fresh provider request offered no old tool.
  The superseded connection is canceled rather than published.

Checks: **280 Rust tests passed**, 129 shell plus 151 engine; **1,186 Bun tests
passed**, 8,422 assertions. Typecheck and workspace/all-target clippy with warnings
denied passed. Probe data was local and disposable, with fake provider credentials
and local MCP endpoints; no live provider inference was used.

**M2 code/automated review is cleared within the agreed scope. Carry on to M3.**
The documented Gemini live-API verification and desktop smoke-test limitations
remain verification notes; async questions, reconnect/reload and lifecycle work
remain M3 tasks. This is not production validation of every provider/account route.
The earlier open-blocker statements below are preserved as historical evidence,
not additional requests to redo these fixes. No runtime source was changed.

## Follow-up at 4630b7fe

Rechecked `4630b7fe9667c21916c217c010194d0b022f95d4` on 2026-09-29, with
catalog/event changes in progress. New commits improve OAuth presentation,
sign-in error reporting and long-secret storage in the Windows keychain. They
do not change the tool dispatch or MCP connection paths behind the two findings.

Rebuilt the headless engine and reran the same local probes:

- Plan request offered no write tool, but the returned write still created a file.
- Replacing MCP config during initialize still left the old tool listed and offered
  while the new config showed `needs_approval`.

Both M2 blockers below remain open. Workspace tests passed: **278**, 129 shell and
149 engine. Typecheck and 25 selected native frontend tests passed. The workspace
suite includes a synthetic real-keychain round-trip test; the probes use fake
credentials and do not access production provider accounts. No runtime files were
modified by this check. M1 sign-off remains unchanged.

## Progress

- The stalled config test is fixed by cloning request data instead of retaining
  the request-log mutex while starting the next turn. It now passes.
- Workspace `drift.json`, custom agents/commands, skill discovery and the skill
  tool exist, with tests and config/command routes.
- MCP stdio/streamable HTTP, saved definitions, exact-config approval, connection
  status and dynamic tool exposure are implemented.
- Formatter hooks have landed with built-in detection, config overrides and a
  post-write integration test.
- Provider-page methods now include ChatGPT and Anthropic Console sign-in.
- The updated checklist explicitly moves MCP reconnect/reload and async questions
  to M3. Respect that split; those are not additional M2 blockers in this review.

## Two corrections before M2 sign-off

### 1. Enforce the run's offered tool set at dispatch

The runner filters advertised schemas using the selected agent's tool list in
`session/turn.rs:241-245`. Dispatch later resolves names through the global
registry rather than enforcing that filtered set. A model can call a known tool
that this run did not offer, including a call named in earlier conversation history.

A local fake-provider probe created a plan session with an explicit edit allow
policy, then returned a valid write call. The outbound request did not offer write,
but the tool created `plan-mutated.txt`. This demonstrates that plan restrictions
are prompt/schema filtering, not execution restrictions.

Pin the allowed executable tool IDs for the run and reject calls outside that set
before asking permission, snapshotting or dispatching. Keep ordinary permission
checks afterward. Add a returned-unoffered-tool fixture, not only assertions that
the request's tool list omits write.

### 2. Fence late MCP connection publication

`mcp/mod.rs:138-160` validates the copied row before awaiting connection, then
publishes it without checking whether the stored config/connection generation
changed. `api/mcp.rs:24-35` disconnects and replaces a definition, but cannot
remove a connection that has not completed yet. The old connect can subsequently
repopulate live state and the dynamic registry.

A delayed local stdio MCP fixture began connecting under an approved config.
During its initialize wait, the probe replaced the config with an unapproved new
command. After the first connect finished, status showed the new command and
`needs_approval`, but still listed `old_tool`. A fresh provider request offered
`probe_old_tool` to the model.

Use per-server connection ownership/revision. Save, disable, disconnect and remove
invalidate pending connection generations; a late result cannot publish tools.
Verify approval against the current effective definition before activating it.
Close rejected late services. This narrow fix does not require M3 automatic reconnect
or full runtime-config leases. Add delayed-initialize fixtures for edit/disconnect
during connect.

## Verification

- Rust workspace run: **277 passed**, 129 shell plus 148 engine tests; headless and
  doc-test targets had zero tests.
- Full Bun suite: **1,186 passed**, 8,422 assertions.
- Application typecheck and workspace/all-target clippy with warnings denied: passed.
- Rebuilt the headless engine and ran both probes through HTTP against local fake
  Anthropic and MCP endpoints. No real account, keychain or paid model requests.
  Disposable workspace/data and captured child processes were cleaned up.
- The probe helpers remain outside git in the approved temp directory as
  `drift-m2-check.ts` and `drift-m2-slow-mcp.cjs`.

Assessment: M2 is advancing and the earlier test stall is resolved. Fix these two
current-feature boundaries before calling M2 signed off. Leave the explicitly
deferred async/reconnect/lifecycle work in M3; do not reopen M1.
