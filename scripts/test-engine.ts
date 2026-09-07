import path from "node:path"
import { engineUpstream, withEngineOverlays } from "./engine-overlays"

function engineEnvironment(extra: Record<string, string> = {}) {
  const env = { ...process.env }
  for (const key of [
    "DRIFT_MCP_APPROVAL_REQUIRED",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_SERVER_PASSWORD",
    "OPENCODE_SERVER_USERNAME",
  ])
    delete env[key]
  // Bootstrap forks a models.dev catalog refresh that lands inside the first test's timeout.
  return { ...env, OPENCODE_DISABLE_MODELS_FETCH: "1", ...extra }
}

// Instance bootstrap costs ~20s locally and ~50s on CI runners, so this only guards against hangs.
const testTimeoutMs = 180_000

async function run(directory: string, args: string[], env?: Record<string, string>) {
  const child = Bun.spawn([process.execPath, "test", "--timeout", String(testTimeoutMs), ...args], {
    cwd: path.join(engineUpstream, directory),
    env: engineEnvironment(env),
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  })
  const exitCode = await child.exited
  if (exitCode !== 0) throw new Error(`Engine tests failed in ${directory}`)
}

async function typecheck(directory: string) {
  const child = Bun.spawn([process.execPath, "run", "typecheck"], {
    cwd: path.join(engineUpstream, directory),
    env: engineEnvironment(),
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  })
  if ((await child.exited) !== 0) throw new Error(`Engine typecheck failed in ${directory}`)
}

async function verifyAuthCapture() {
  const script = [
    'import { Effect } from "effect"',
    'import { ServerAuth } from "./src/server/auth"',
    "const config = await Effect.runPromise(ServerAuth.Config.pipe(Effect.provide(ServerAuth.layer)))",
    'if (!ServerAuth.required(config)) throw new Error("Drift auth password was not captured")',
    'if (process.env.OPENCODE_SERVER_PASSWORD !== undefined) throw new Error("Drift auth password remained in process.env")',
  ].join(";")
  const child = Bun.spawn([process.execPath, "-e", script], {
    cwd: path.join(engineUpstream, "packages", "opencode"),
    env: engineEnvironment({ DRIFT_MCP_APPROVAL_REQUIRED: "1", OPENCODE_SERVER_PASSWORD: "secret" }),
    stdout: "inherit",
    stderr: "inherit",
  })
  if ((await child.exited) !== 0) throw new Error("Engine auth capture verification failed")
}

await withEngineOverlays(async () => {
  await typecheck("packages/opencode")
  await typecheck("packages/core")
  await typecheck("packages/schema")
  await run("packages/opencode", [
    "test/question/question.test.ts",
    "test/question/async-question.test.ts",
    "test/tool/question.test.ts",
    "test/effect/runner.test.ts",
    "test/effect/async-question.test.ts",
  ])
  await run("packages/opencode", ["test/session/prompt.test.ts", "-t", "async question"])
  await run("packages/opencode", ["test/session/instruction.test.ts"])
  await run("packages/core", ["test/event.test.ts"])
  await run("packages/opencode", ["test/plugin/codex.test.ts"])
  await run("packages/opencode", [
    "test/config/v2-compat.test.ts",
    "test/config/v2-mcp-compat.test.ts",
    "test/provider/header-timeout.test.ts",
    "test/provider/transform.test.ts",
    "test/plugin/azure.test.ts",
    "test/plugin/github-copilot.test.ts",
    "test/session/tools.test.ts",
    "test/patched-dependencies.test.ts",
  ])
  await run("packages/core", ["test/database-migration.test.ts"])
  await run("packages/core", ["test/move-session.test.ts"])
  await run("packages/core", ["test/session-compaction.test.ts"])
  await run("packages/opencode", ["test/server/httpapi-control-plane.test.ts"])
  await verifyAuthCapture()
  await run("packages/opencode", [
    "test/project/instance-bootstrap.test.ts",
    "-t",
    "InstanceStore|CLI bootstrap|Drift requires|mutable synthetic|changed after sealing",
  ])
  await run("packages/opencode", [
    "test/provider/provider.test.ts",
    "-t",
    "LM Studio discovers|provider reload invalidates",
  ])
  await run("packages/opencode", ["test/tool/registry.test.ts", "-t", "LM Studio without global code mode"])
  await run("packages/opencode", ["test/server/httpapi-instance-route-auth.test.ts"])
  await run("packages/opencode", ["test/tool/shell.test.ts", "-t", "terminates command on timeout"])
  await run("packages/opencode", ["test/tool/shell-timeout.test.ts"])
  await run("packages/opencode", ["test/tool/shell-output-throttle.test.ts"])
  await run("packages/opencode", ["test/server/sse-backpressure.test.ts"])
  await run("packages/opencode", ["test/session/compaction-scan.test.ts"])
  await run("packages/opencode", ["test/mcp/lifecycle.test.ts", "-t", "required Drift mode"], {
    DRIFT_MCP_APPROVAL_REQUIRED: "1",
  })
  await run("packages/opencode", ["test/session/messages-pagination.test.ts", "-t", "active fork"])
  await run("packages/opencode", ["test/session/compaction-row-scan.test.ts"])
  await run("packages/opencode", [
    "test/mcp/lifecycle.test.ts",
    "-t",
    "restores tools|newer reconnect wins|ordinary MCP request failures|reload\\(\\) picks up config edits",
  ])
  await run("packages/opencode", [
    "test/session/compaction.test.ts",
    "-t",
    "continues from compacted context|uses a hidden continuation|summarizes only the head|anchors repeated compactions|keeps plugin context",
  ])
  await run("packages/opencode", [
    "test/session/processor-effect.test.ts",
    "-t",
    "compact on structured context overflow",
  ])
  await run("packages/opencode", ["test/session/retry-model-switch.test.ts"])
})
