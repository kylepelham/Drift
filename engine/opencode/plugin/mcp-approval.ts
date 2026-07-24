import { createHash } from "node:crypto"
import { mkdir, rename, rm, writeFile } from "node:fs/promises"
import path from "node:path"
import type { Plugin, PluginOptions } from "@opencode-ai/plugin"

type McpDefinition = Record<string, unknown> & { type: "local" | "remote" }
type Decision = "approved" | "rejected"
type Policy = {
  version: number
  generation: number
  decisions: Array<{ fingerprint: string; decision: Decision }>
}
type Options = { policyPath: string; pendingDirectory: string; sentinelPath: string; generation: number }

const gate = Symbol.for("drift.mcp.approval.gate")

export const McpApproval: Plugin = async (input, options) => {
  // Server auth is captured before instance plugins run. MCP stdio children must not inherit it.
  delete process.env.OPENCODE_SERVER_PASSWORD

  return {
    async config(config) {
      const mcp = config.mcp as Record<string, unknown> | undefined
      if (!mcp) return seal(config)

      const definitions = Object.entries(mcp).filter(
        (entry): entry is [string, McpDefinition] => isDefinition(entry[1]),
      )
      for (const name of Object.keys(mcp)) mcp[name] = { enabled: false }
      if (!definitions.length) return seal(config)

      const settings = readOptions(options)
      if (!settings) return seal(config)
      if (await Bun.file(settings.sentinelPath).exists()) {
        await writeReport(settings, input.directory, [])
        return seal(config)
      }
      const policy = await readPolicy(settings).catch(() => undefined)
      if (!policy) {
        await writeReport(settings, input.directory, [])
        return seal(config)
      }

      const decisions = new Map(policy.decisions.map((item) => [item.fingerprint, item.decision]))
      const observed = definitions.map(([name, definition]) => {
        const fingerprint = fingerprintFor(name, definition)
        return {
          name,
          type: definition.type,
          fingerprint,
          decision: validDefinition(definition) ? (decisions.get(fingerprint) ?? ("pending" as const)) : ("invalid" as const),
        }
      })
      if (observed.some((item) => !item.fingerprint)) return seal(config)

      const reported = await writeReport(settings, input.directory, observed)
      if (!reported) return seal(config)

      for (let index = 0; index < definitions.length; index++) {
        if (observed[index].decision === "approved") mcp[definitions[index][0]] = definitions[index][1]
      }
      seal(config)
    },
  }
}

async function writeReport(settings: Options, directory: string, servers: unknown[]) {
  const destination = path.join(
    settings.pendingDirectory,
    `${createHash("sha256").update(normalizeDirectory(directory)).digest("hex")}.json`,
  )
  const temporary = `${destination}.${process.pid}.${crypto.randomUUID()}.tmp`
  return mkdir(settings.pendingDirectory, { recursive: true })
    .then(() =>
      writeFile(
        temporary,
        JSON.stringify({ version: 3, generation: settings.generation, directory, servers }),
      ),
    )
    .then(() => rename(temporary, destination))
    .then(() => true)
    .catch(() => false)
    .finally(() => rm(temporary, { force: true }).catch(() => undefined))
}

function seal(config: object) {
  const mcp = (config as { mcp?: unknown }).mcp
  const expected = canonical(mcp)
  Object.defineProperty(config, gate, {
    configurable: false,
    writable: false,
    value: (current: { mcp?: unknown }) =>
      mcp === undefined ? current.mcp === undefined : expected !== undefined && canonical(current.mcp) === expected,
  })
}

function readOptions(options?: PluginOptions): Options | undefined {
  if (!options) return
  if (
    typeof options.policyPath !== "string" ||
    typeof options.pendingDirectory !== "string" ||
    typeof options.sentinelPath !== "string"
  ) return
  if (!Number.isSafeInteger(options.generation) || (options.generation as number) < 0) return
  return {
    policyPath: options.policyPath,
    pendingDirectory: options.pendingDirectory,
    sentinelPath: options.sentinelPath,
    generation: options.generation as number,
  }
}

async function readPolicy(options: Options): Promise<Policy | undefined> {
  const policy = (await Bun.file(options.policyPath).json()) as Partial<Policy>
  if (!hasOnlyKeys(policy, ["decisions", "generation", "version"])) return
  if (policy.version !== 3 || policy.generation !== options.generation || !Array.isArray(policy.decisions)) return
  const decisions = policy.decisions.filter(
    (item): item is Policy["decisions"][number] =>
      hasOnlyKeys(item, ["decision", "fingerprint"]) &&
      typeof item?.fingerprint === "string" &&
      /^sha256:[0-9a-f]{64}$/.test(item.fingerprint) &&
      (item.decision === "approved" || item.decision === "rejected"),
  )
  if (decisions.length !== policy.decisions.length) return
  if (new Set(decisions.map((item) => item.fingerprint)).size !== decisions.length) return
  return { version: 3, generation: options.generation, decisions }
}

function hasOnlyKeys(value: unknown, expected: string[]) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false
  const keys = Object.keys(value).sort()
  return keys.length === expected.length && keys.every((key, index) => key === expected[index])
}

function isDefinition(value: unknown): value is McpDefinition {
  if (!plainObject(value)) return false
  return (value as McpDefinition).type === "local" || (value as McpDefinition).type === "remote"
}

function validDefinition(definition: McpDefinition) {
  if (definition.enabled !== undefined && typeof definition.enabled !== "boolean") return false
  if (
    definition.timeout !== undefined &&
    (!Number.isSafeInteger(definition.timeout) || (definition.timeout as number) <= 0)
  ) return false
  if (definition.type === "local") {
    if (!Array.isArray(definition.command) || definition.command.length === 0) return false
    if (definition.command.some((item) => typeof item !== "string")) return false
    if (!(definition.command[0] as string).trim()) return false
    if (definition.cwd !== undefined && typeof definition.cwd !== "string") return false
    return definition.environment === undefined || stringRecord(definition.environment)
  }
  if (typeof definition.url !== "string") return false
  const remote = safeUrl(definition.url)
  if (!remote || (remote.protocol !== "http:" && remote.protocol !== "https:")) return false
  if (definition.headers !== undefined && !stringRecord(definition.headers)) return false
  if (definition.oauth === undefined || definition.oauth === false) return true
  if (!plainObject(definition.oauth)) return false
  const oauth = definition.oauth as Record<string, unknown>
  if (["clientId", "clientSecret", "scope", "redirectUri"].some((key) => oauth[key] !== undefined && typeof oauth[key] !== "string")) return false
  return oauth.callbackPort === undefined || (Number.isInteger(oauth.callbackPort) && (oauth.callbackPort as number) >= 1 && (oauth.callbackPort as number) <= 65535)
}

function plainObject(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false
  const prototype = Object.getPrototypeOf(value)
  return prototype === Object.prototype || prototype === null
}

function stringRecord(value: unknown) {
  return plainObject(value) && Object.values(value).every((item) => typeof item === "string")
}

function safeUrl(value: string) {
  try {
    return new URL(value)
  } catch {
    return undefined
  }
}

function fingerprintFor(name: string, definition: McpDefinition) {
  const effective = Object.fromEntries(Object.entries(definition).filter(([key]) => key !== "enabled"))
  const serialized = canonical([name, effective])
  if (serialized === undefined) return ""
  return `sha256:${createHash("sha256").update(serialized).digest("hex")}`
}

function normalizeDirectory(value: string) {
  const normalized = value.replaceAll("\\", "/").replace(/\/+$/, "")
  return process.platform === "win32"
    ? normalized.replace(/[A-Z]/g, (character) => String.fromCharCode(character.charCodeAt(0) + 32))
    : normalized
}

function canonical(value: unknown, seen = new Set<object>()): string | undefined {
  if (value === null || typeof value === "string" || typeof value === "boolean") return JSON.stringify(value)
  if (typeof value === "number") return Number.isFinite(value) ? JSON.stringify(value) : undefined
  if (Array.isArray(value)) {
    if (seen.has(value)) return
    seen.add(value)
    const items = value.map((item) => canonical(item, seen))
    seen.delete(value)
    return items.some((item) => item === undefined) ? undefined : `[${items.join(",")}]`
  }
  if (!value || typeof value !== "object") return
  if (seen.has(value)) return
  seen.add(value)
  const entries = Object.entries(value as Record<string, unknown>).sort(([left], [right]) =>
    left < right ? -1 : left > right ? 1 : 0,
  )
  const items = entries.map(([key, item]) => {
    const encoded = canonical(item, seen)
    return encoded === undefined ? undefined : `${JSON.stringify(key)}:${encoded}`
  })
  seen.delete(value)
  return items.some((item) => item === undefined) ? undefined : `{${items.join(",")}}`
}
