import type { McpConfig } from "./store"

export type McpPair = { key: string; value: string }
export type McpOAuthMode = "auto" | "disabled" | "configured"
export type McpFormState = {
  type: "local" | "remote"
  command: string[]
  cwd: string
  environment: McpPair[]
  url: string
  headers: McpPair[]
  enabled: boolean
  timeout: string
  oauthMode: McpOAuthMode
  clientId: string
  clientSecret: string
  scope: string
  callbackPort: string
  redirectUri: string
  extra: Record<string, unknown>
  oauthExtra: Record<string, unknown>
  present: Set<string>
  oauthPresent: Set<string>
}

export type McpFormIssue =
  | "commandRequired"
  | "urlRequired"
  | "urlInvalid"
  | "timeoutInvalid"
  | "pairInvalid"
  | "callbackPortInvalid"
  | "redirectUriInvalid"
export type McpFormResult = { config: McpConfig; issue?: never } | { config?: never; issue: McpFormIssue }

export function mcpFormState(config?: McpConfig): McpFormState {
  const current = config ?? { type: "local" }
  const oauth = recordValue(current.oauth)
  const known = new Set(
    current.type === "local"
      ? ["type", "command", "cwd", "environment", "enabled", "timeout"]
      : ["type", "url", "headers", "oauth", "enabled", "timeout"],
  )
  const oauthKnown = new Set(["clientId", "clientSecret", "scope", "callbackPort", "redirectUri"])
  return {
    type: current.type,
    command:
      Array.isArray(current.command) && current.command.every((item) => typeof item === "string")
        ? [...current.command]
        : [""],
    cwd: stringValue(current.cwd),
    environment: pairsValue(current.environment),
    url: stringValue(current.url),
    headers: pairsValue(current.headers),
    enabled: current.enabled !== false,
    timeout: typeof current.timeout === "number" ? String(current.timeout) : "",
    oauthMode: current.oauth === false ? "disabled" : oauth ? "configured" : "auto",
    clientId: stringValue(oauth?.clientId),
    clientSecret: stringValue(oauth?.clientSecret),
    scope: stringValue(oauth?.scope),
    callbackPort: typeof oauth?.callbackPort === "number" ? String(oauth.callbackPort) : "",
    redirectUri: stringValue(oauth?.redirectUri),
    extra: Object.fromEntries(Object.entries(current).filter(([key]) => !known.has(key))),
    oauthExtra: Object.fromEntries(Object.entries(oauth ?? {}).filter(([key]) => !oauthKnown.has(key))),
    present: new Set(Object.keys(current)),
    oauthPresent: new Set(Object.keys(oauth ?? {})),
  }
}

export function mcpConfigFromForm(form: McpFormState): McpFormResult {
  const timeout = positiveInteger(form.timeout)
  if (timeout === null) return { issue: "timeoutInvalid" }
  const shared = {
    ...(form.present.has("enabled") ? { enabled: form.enabled } : {}),
    ...(form.present.has("timeout") && timeout !== undefined ? { timeout } : {}),
  }
  if (form.type === "local") {
    if (!form.command[0]?.trim()) return { issue: "commandRequired" }
    const environment = pairRecord(form.environment)
    if (!environment) return { issue: "pairInvalid" }
    return {
      config: {
        ...form.extra,
        type: "local",
        command: [...form.command],
        ...(form.present.has("cwd") ? { cwd: form.cwd } : {}),
        ...(form.present.has("environment") ? { environment } : {}),
        ...shared,
      },
    }
  }
  if (!form.url) return { issue: "urlRequired" }
  if (!mcpRemoteUrlAllowed(form.url)) return { issue: "urlInvalid" }
  const headers = pairRecord(form.headers)
  if (!headers) return { issue: "pairInvalid" }
  const callbackPort = positiveInteger(form.callbackPort)
  if (callbackPort === null || (callbackPort !== undefined && callbackPort > 65535)) {
    return { issue: "callbackPortInvalid" }
  }
  if (form.redirectUri && !mcpRemoteUrlAllowed(form.redirectUri)) return { issue: "redirectUriInvalid" }
  const oauth =
    form.oauthMode === "disabled"
      ? { oauth: false }
      : form.oauthMode === "configured"
        ? {
            oauth: {
              ...form.oauthExtra,
              ...(form.oauthPresent.has("clientId") ? { clientId: form.clientId } : {}),
              ...(form.oauthPresent.has("clientSecret") ? { clientSecret: form.clientSecret } : {}),
              ...(form.oauthPresent.has("scope") ? { scope: form.scope } : {}),
              ...(form.oauthPresent.has("callbackPort") && callbackPort !== undefined ? { callbackPort } : {}),
              ...(form.oauthPresent.has("redirectUri") ? { redirectUri: form.redirectUri } : {}),
            },
          }
        : {}
  return {
    config: {
      ...form.extra,
      type: "remote",
      url: form.url,
      ...(form.present.has("headers") ? { headers } : {}),
      ...oauth,
      ...shared,
    },
  }
}

export function mcpRemoteUrlAllowed(value: string) {
  try {
    const url = new URL(value)
    return url.protocol === "http:" || url.protocol === "https:"
  } catch {
    return false
  }
}

export function withMcpPresence(form: McpFormState, field: string, present: boolean) {
  const fields = new Set(form.present)
  if (present) fields.add(field)
  else fields.delete(field)
  return { ...form, present: fields }
}

export function withMcpOAuthPresence(form: McpFormState, field: string, present: boolean) {
  const fields = new Set(form.oauthPresent)
  if (present) fields.add(field)
  else fields.delete(field)
  return { ...form, oauthPresent: fields }
}

function recordValue(value: unknown) {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined
}

function stringValue(value: unknown) {
  return typeof value === "string" ? value : ""
}

function pairsValue(value: unknown): McpPair[] {
  const record = recordValue(value)
  return record
    ? Object.entries(record).flatMap(([key, item]) => (typeof item === "string" ? [{ key, value: item }] : []))
    : []
}

function pairRecord(pairs: McpPair[]) {
  const entries = pairs.filter((item) => item.key || item.value)
  if (entries.some((item) => !item.key) || new Set(entries.map((item) => item.key)).size !== entries.length) return null
  return Object.fromEntries(entries.map((item) => [item.key, item.value]))
}

function positiveInteger(value: string) {
  if (!value.trim()) return undefined
  if (!/^\d+$/.test(value.trim())) return null
  const number = Number(value)
  return Number.isSafeInteger(number) && number > 0 ? number : null
}
