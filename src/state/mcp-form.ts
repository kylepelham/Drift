import type { McpServerConfig } from "../engine/store"

export type McpPair = { key: string; value: string }
/** Exactly what the engine's server config holds: a command with its arguments and environment, or a URL with headers. */
export type McpFormState = {
  type: "stdio" | "http"
  command: string[]
  environment: McpPair[]
  url: string
  headers: McpPair[]
}

export type McpFormIssue = "commandRequired" | "urlRequired" | "urlInvalid" | "pairInvalid"
export type McpFormResult = { config: McpServerConfig; issue?: never } | { config?: never; issue: McpFormIssue }

export function mcpFormState(config?: McpServerConfig): McpFormState {
  if (config?.type === "http") {
    return { type: "http", command: [""], environment: [], url: config.url, headers: pairs(config.headers) }
  }
  return {
    type: "stdio",
    command: config ? [config.command, ...(config.args ?? [])] : [""],
    environment: pairs(config?.env),
    url: "",
    headers: [],
  }
}

export function mcpConfigFromForm(form: McpFormState): McpFormResult {
  if (form.type === "stdio") {
    const [command, ...args] = form.command
    if (!command?.trim()) return { issue: "commandRequired" }
    const env = pairRecord(form.environment)
    if (!env) return { issue: "pairInvalid" }
    return { config: { type: "stdio", command, args, env } }
  }
  if (!form.url) return { issue: "urlRequired" }
  if (!mcpRemoteUrlAllowed(form.url)) return { issue: "urlInvalid" }
  const headers = pairRecord(form.headers)
  if (!headers) return { issue: "pairInvalid" }
  return { config: { type: "http", url: form.url, headers } }
}

export function mcpRemoteUrlAllowed(value: string) {
  try {
    const url = new URL(value)
    return url.protocol === "http:" || url.protocol === "https:"
  } catch {
    return false
  }
}

function pairs(record: Record<string, string> | undefined): McpPair[] {
  return Object.entries(record ?? {}).map(([key, value]) => ({ key, value }))
}

function pairRecord(entries: McpPair[]) {
  const filled = entries.filter((item) => item.key || item.value)
  if (filled.some((item) => !item.key) || new Set(filled.map((item) => item.key)).size !== filled.length) return null
  return Object.fromEntries(filled.map((item) => [item.key, item.value]))
}
