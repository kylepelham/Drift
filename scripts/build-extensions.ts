import { cpSync, mkdirSync, readFileSync, rmSync } from "node:fs"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
const source = path.join(root, "engine", "opencode")
const defaultOutput = path.join(root, "src-tauri", "generated", "drift-extensions")

const promptFamilies = ["meta", "beast", "codex", "gpt", "gemini", "anthropic", "trinity", "kimi", "default"]
const agentPrompts = ["explore", "compaction", "title", "summary"]

export function driftIdentity(prompt: string) {
  return prompt
    .replace(/^You are OpenCode\b/, "You are Drift")
    .replace(/^You are opencode\b/, "You are Drift")
    .replace("identify yourself as OpenCode powered by", "identify yourself as Drift powered by")
    .replace("When the user directly asks about OpenCode", "When the user directly asks about Drift")
    .replace("When the user directly asks about opencode", "When the user directly asks about Drift")
}

export function promptCatalog() {
  const sessionPrompts = path.join(root, "engine", "upstream", "packages", "opencode", "src", "session", "prompt")
  const agentSource = path.join(root, "engine", "upstream", "packages", "opencode", "src", "agent", "prompt")
  return {
    version: 1,
    families: promptFamilies.map((id) => {
      const original = readFileSync(path.join(sessionPrompts, `${id}.txt`), "utf8").trim()
      return { id, original, default: driftIdentity(original) }
    }),
    agents: agentPrompts.map((name) => ({
      name,
      prompt: readFileSync(path.join(agentSource, `${name}.txt`), "utf8").trim(),
    })),
  }
}

export async function buildExtensions(output = defaultOutput) {
  const plugins = path.join(output, "plugin")
  mkdirSync(output, { recursive: true })
  rmSync(plugins, { recursive: true, force: true })
  rmSync(path.join(output, "opencode.json"), { force: true })
  rmSync(path.join(output, "package.json"), { force: true })
  mkdirSync(plugins, { recursive: true })
  const result = await Bun.build({
    entrypoints: [
      path.join(source, "plugin", "spawn-thread.ts"),
      path.join(source, "plugin", "mcp-approval.ts"),
      path.join(source, "plugin", "prompt-overrides.ts"),
    ],
    outdir: plugins,
    target: "bun",
    format: "esm",
    minify: true,
  })
  if (!result.success) throw new AggregateError(result.logs, "failed to bundle Drift engine extensions")
  cpSync(path.join(source, "opencode.json"), path.join(output, "opencode.json"))
  await Bun.write(path.join(output, "prompt-catalog.json"), `${JSON.stringify(promptCatalog(), null, 2)}\n`)
  await Bun.write(
    path.join(output, "package.json"),
    `${JSON.stringify({ name: "drift-engine-extensions", private: true, type: "module" }, null, 2)}\n`,
  )
  return output
}

if (import.meta.main) {
  const output = await buildExtensions(defaultOutput)
  console.log(`drift extensions ready in ${path.relative(root, output)}`)
}
