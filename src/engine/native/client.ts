// Typed access to the native engine. Request and response shapes come from the generated types.
import type { components, operations } from "./types"

export type Target = { url: string; token: string }
export type Workspace = components["schemas"]["Workspace"]
export type NewWorkspace = components["schemas"]["NewWorkspace"]
export type Health = components["schemas"]["Health"]
export type Frame = components["schemas"]["Frame"]
export type Envelope = components["schemas"]["Envelope"]
export type Event = components["schemas"]["Event"]
export type Session = components["schemas"]["Session"]
export type MessageWithParts = components["schemas"]["MessageWithParts"]
export type Prompt = components["schemas"]["Prompt"]
export type Receipt = components["schemas"]["Receipt"]
export type PermissionRequest = components["schemas"]["Request"]
export type ReplyBody = components["schemas"]["ReplyBody"]
export type ProviderStatus = components["schemas"]["ProviderStatus"]
export type ErrorBody = components["schemas"]["ErrorBody"]

type Json<Op extends keyof operations, Status extends number> = operations[Op]["responses"] extends Record<
  Status,
  { content: { "application/json": infer Body } }
>
  ? Body
  : never

export class EngineError extends Error {
  constructor(
    readonly status: number,
    readonly path: string,
    readonly code?: string,
    detail?: string,
  ) {
    super(detail ?? `engine ${status} on ${path}`)
  }
}

export function createClient(target: Target) {
  const request = async <T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> => {
    const response = await fetch(`${target.url}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${target.token}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    })
    if (!response.ok) {
      const error = (await response.json().catch(() => null)) as ErrorBody | null
      throw new EngineError(response.status, path, error?.code, error?.message)
    }
    if (response.status === 204) return undefined as T
    return response.json() as Promise<T>
  }
  const query = (params: Record<string, string | number | boolean | null | undefined>) => {
    const search = new URLSearchParams()
    for (const [key, value] of Object.entries(params)) if (value !== undefined && value !== null) search.set(key, String(value))
    const text = search.toString()
    return text ? `?${text}` : ""
  }
  return {
    health: () => request<Json<"health", 200>>("GET", "/health"),
    workspaces: () => request<Json<"listWorkspaces", 200>>("GET", "/workspaces"),
    createWorkspace: (body: NewWorkspace) => request<Json<"createWorkspace", 201>>("POST", "/workspaces", body),
    sessions: (params: operations["listSessions"]["parameters"]["query"] = {}) =>
      request<Json<"listSessions", 200>>("GET", `/sessions${query({ ...params })}`),
    createSession: (body: components["schemas"]["NewSessionBody"]) => request<Json<"createSession", 201>>("POST", "/sessions", body),
    session: (id: string) => request<Json<"getSession", 200>>("GET", `/sessions/${id}`),
    updateSession: (id: string, body: components["schemas"]["PatchSession"]) =>
      request<Json<"updateSession", 200>>("PATCH", `/sessions/${id}`, body),
    messages: (id: string, params: operations["listMessages"]["parameters"]["query"] = {}, signal?: AbortSignal) =>
      request<Json<"listMessages", 200>>("GET", `/sessions/${id}/messages${query({ ...params })}`, undefined, signal),
    submit: (id: string, prompt: Prompt) => request<Json<"submitTurn", 202>>("POST", `/sessions/${id}/turns`, prompt),
    abort: (id: string) => request<Json<"abortTurn", 200>>("POST", `/sessions/${id}/abort`),
    providers: () => request<Json<"listProviders", 200>>("GET", "/providers"),
    setProviderKey: (id: string, key: string) => request<void>("PUT", `/providers/${id}/key`, { key }),
    removeProviderCredentials: (id: string) => request<void>("DELETE", `/providers/${id}/credentials`),
    startOAuth: (id: string, mode: "max" | "console") => request<Json<"startOAuth", 200>>("POST", `/providers/${id}/oauth`, { mode }),
    finishOAuth: (id: string, input: string) => request<void>("POST", `/providers/${id}/oauth/callback`, { input }),
    permissions: () => request<Json<"listPermissions", 200>>("GET", "/permissions"),
    replyPermission: (id: string, body: ReplyBody) => request<void>("POST", `/permissions/${id}/reply`, body),
  }
}

export type Client = ReturnType<typeof createClient>
