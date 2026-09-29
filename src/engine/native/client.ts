// Typed access to the native engine. Request and response shapes come from the generated types.
import type { components, operations } from "./types"

export type Target = { url: string; token: string }
export type Workspace = components["schemas"]["Workspace"]
export type NewWorkspace = components["schemas"]["NewWorkspace"]
export type Health = components["schemas"]["Health"]
export type Frame = components["schemas"]["Frame"]
export type Envelope = components["schemas"]["Envelope"]
export type Event = components["schemas"]["Event"]

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
  ) {
    super(`engine ${status} on ${path}`)
  }
}

export function createClient(target: Target) {
  const request = async <T>(method: string, path: string, body?: unknown): Promise<T> => {
    const response = await fetch(`${target.url}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${target.token}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    if (!response.ok) throw new EngineError(response.status, path)
    return response.json() as Promise<T>
  }
  return {
    health: () => request<Json<"health", 200>>("GET", "/health"),
    workspaces: () => request<Json<"listWorkspaces", 200>>("GET", "/workspaces"),
    createWorkspace: (body: NewWorkspace) => request<Json<"createWorkspace", 201>>("POST", "/workspaces", body),
  }
}

export type Client = ReturnType<typeof createClient>
