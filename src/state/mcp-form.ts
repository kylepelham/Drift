import type { McpServerConfig, McpServerConfigView } from "../engine/store"

/** `saved`: the engine holds a value under this name, never shown; left empty, it is kept as it is. */
export type McpPair = { key: string; value: string; saved?: boolean }
/** Exactly what the engine's server config holds: a command with its arguments, environment and directory, or a URL with headers; and a call timeout. */
export type McpFormState = {
  type: "stdio" | "http" | "sse"
  command: string[]
  environment: McpPair[]
  cwd: string
  url: string
  headers: McpPair[]
  /** Seconds, as typed; empty means no limit. */
  timeout: string
}

export type McpFormIssue = "commandRequired" | "urlRequired" | "urlInvalid" | "pairInvalid" | "timeoutInvalid"
export type McpFormResult = { config: McpServerConfig; issue?: never } | { config?: never; issue: McpFormIssue }

export function mcpFormState(config?: McpServerConfigView): McpFormState {
  const timeout = config?.timeoutSeconds ? String(config.timeoutSeconds) : ""
  if (config?.type === "http" || config?.type === "sse") {
    return { type: config.type, command: [""], environment: [], cwd: "", url: config.url, headers: savedPairs(config.headers), timeout }
  }
  return {
    type: "stdio",
    command: config ? [config.command, ...config.args] : [""],
    environment: savedPairs(config?.env ?? []),
    cwd: config?.cwd ?? "",
    url: "",
    headers: [],
    timeout,
  }
}

export function mcpConfigFromForm(form: McpFormState): McpFormResult {
  const timeout = form.timeout.trim()
  const timeoutSeconds = timeout ? Number(timeout) : null
  if (timeoutSeconds !== null && !(Number.isInteger(timeoutSeconds) && timeoutSeconds > 0)) return { issue: "timeoutInvalid" }
  if (form.type === "stdio") {
    const [command, ...args] = form.command
    if (!command?.trim()) return { issue: "commandRequired" }
    const env = pairRecord(form.environment)
    if (!env) return { issue: "pairInvalid" }
    return { config: { type: "stdio", command, args, env, cwd: form.cwd.trim() || null, timeoutSeconds } }
  }
  if (!form.url) return { issue: "urlRequired" }
  if (!mcpRemoteUrlAllowed(form.url)) return { issue: "urlInvalid" }
  const headers = pairRecord(form.headers)
  if (!headers) return { issue: "pairInvalid" }
  return { config: { type: form.type, url: form.url, headers, timeoutSeconds } }
}

export function mcpRemoteUrlAllowed(value: string) {
  try {
    const url = new URL(value)
    return url.protocol === "http:" || url.protocol === "https:"
  } catch {
    return false
  }
}

/** A changed name is a new entry: the engine holds nothing under it to keep. */
export function updatePair(pairs: McpPair[], index: number, patch: Partial<McpPair>) {
  return pairs.map((pair, item) => (item === index ? { ...pair, ...patch, ...("key" in patch ? { saved: false } : {}) } : pair))
}

function savedPairs(names: string[]): McpPair[] {
  return names.map((key) => ({ key, value: "", saved: true }))
}

function pairRecord(entries: McpPair[]): Record<string, string | null> | null {
  const filled = entries.filter((item) => item.key || item.value || item.saved)
  if (filled.some((item) => !item.key) || new Set(filled.map((item) => item.key)).size !== filled.length) return null
  return Object.fromEntries(filled.map((item) => [item.key, item.saved && !item.value ? null : item.value]))
}
