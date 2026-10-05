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
export type PermissionRequest = components["schemas"]["PermissionRequest"]
export type PermissionRule = components["schemas"]["Rule"]
export type QuestionRequest = components["schemas"]["QuestionRequest"]
export type Todo = components["schemas"]["Todo"]
export type TaskRecord = components["schemas"]["TaskRecord"]
export type ReplyBody = components["schemas"]["ReplyBody"]
export type ProviderStatus = components["schemas"]["ProviderStatus"]
export type ErrorBody = components["schemas"]["ErrorBody"]
export type McpServerStatus = components["schemas"]["ServerStatus"]
/** What the UI sends: a `null` env or header value keeps the saved one. */
export type McpServerConfig = components["schemas"]["ServerConfigInput"]
/** What the UI sees: env and header names, never their values. */
export type McpServerConfigView = components["schemas"]["ServerConfigView"]
export type WorkspaceConfig = components["schemas"]["Config"]

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
    const text = await response.text()
    return (text ? JSON.parse(text) : undefined) as T
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
    workspaceConfig: (id: string) => request<Json<"workspaceConfig", 200>>("GET", `/workspaces/${id}/config`),
    runCommand: (id: string, name: string, args: string) => request<Json<"runCommand", 202>>("POST", `/sessions/${id}/command`, { name, arguments: args }),
    createWorkspace: (body: NewWorkspace) => request<Json<"createWorkspace", 201>>("POST", "/workspaces", body),
    sessions: (params: operations["listSessions"]["parameters"]["query"] = {}) =>
      request<Json<"listSessions", 200>>("GET", `/sessions${query({ ...params })}`),
    createSession: (body: components["schemas"]["NewSessionBody"]) => request<Json<"createSession", 201>>("POST", "/sessions", body),
    spawnThread: (id: string, instruction: string) => request<Json<"spawnThread", 201>>("POST", `/sessions/${id}/spawn`, { instruction }),
    forkSession: (id: string, atMessage?: string) => request<Json<"forkSession", 201>>("POST", `/sessions/${id}/fork`, { atMessage }),
    moveSession: (id: string, workspaceId: string) => request<Json<"moveSession", 200>>("POST", `/sessions/${id}/move`, { workspaceId }),
    compactSession: (id: string) => request<void>("POST", `/sessions/${id}/compact`),
    switchRetryModel: (id: string, model: components["schemas"]["ModelRef"], variant: string | null) =>
      request<void>("POST", `/sessions/${id}/retry`, { model, variant }),
    revertSession: (id: string, messageId: string) => request<Json<"revertSession", 200>>("POST", `/sessions/${id}/revert`, { messageId }),
    unrevertSession: (id: string) => request<Json<"unrevertSession", 200>>("POST", `/sessions/${id}/unrevert`),
    settings: () => request<Json<"getSettings", 200>>("GET", "/settings"),
    putSettings: (body: components["schemas"]["EngineSettings"]) => request<Json<"putSettings", 200>>("PUT", "/settings", body),
    session: (id: string) => request<Json<"getSession", 200>>("GET", `/sessions/${id}`),
    updateSession: (id: string, body: components["schemas"]["PatchSession"]) =>
      request<Json<"updateSession", 200>>("PATCH", `/sessions/${id}`, body),
    deleteSession: (id: string) => request<void>("DELETE", `/sessions/${id}`),
    purgeArchivedSession: (id: string) => request<void>("DELETE", `/sessions/${id}?archived=true`),
    purgeWorkspace: (id: string) => request<Json<"purgeWorkspace", 200>>("POST", `/workspaces/${id}/purge`),
    messages: (id: string, params: operations["listMessages"]["parameters"]["query"] = {}, signal?: AbortSignal) =>
      request<Json<"listMessages", 200>>("GET", `/sessions/${id}/messages${query({ ...params })}`, undefined, signal),
    submit: (id: string, prompt: Prompt) => request<Json<"submitTurn", 202>>("POST", `/sessions/${id}/turns`, prompt),
    findFiles: (workspaceId: string, text: string, signal?: AbortSignal) =>
      request<Json<"findFiles", 200>>("GET", `/workspaces/${workspaceId}/files${query({ query: text })}`, undefined, signal),
    abort: (id: string) => request<Json<"abortTurn", 200>>("POST", `/sessions/${id}/abort`),
    providers: () => request<Json<"listProviders", 200>>("GET", "/providers"),
    setProviderKey: (id: string, key: string) => request<void>("PUT", `/providers/${id}/key`, { key }),
    removeProviderCredentials: (id: string) => request<void>("DELETE", `/providers/${id}/credentials`),
    startOAuth: (id: string, mode: "max" | "console" | "chatgpt" | "supergrok") => request<Json<"startOAuth", 200>>("POST", `/providers/${id}/oauth`, { mode }),
    finishOAuth: (id: string, input: string, state?: string) => request<void>("POST", `/providers/${id}/oauth/callback`, { input, state }),
    permissions: () => request<Json<"listPermissions", 200>>("GET", "/permissions"),
    replyPermission: (id: string, body: ReplyBody) => request<void>("POST", `/permissions/${id}/reply`, body),
    questions: () => request<Json<"listQuestions", 200>>("GET", "/questions"),
    answerQuestion: (id: string, answers: string[][]) => request<void>("POST", `/questions/${id}/reply`, { answers }),
    rejectQuestion: (id: string) => request<void>("POST", `/questions/${id}/reject`),
    mcpServers: () => request<Json<"listMcpServers", 200>>("GET", "/mcp"),
    saveMcpServer: (name: string, config: McpServerConfig, options: { create?: boolean; readOnlyTrusted?: boolean } = {}) => {
      const query = new URLSearchParams()
      if (options.create) query.set("create", "true")
      if (options.readOnlyTrusted !== undefined) query.set("readOnlyTrusted", String(options.readOnlyTrusted))
      const search = query.size ? `?${query}` : ""
      return request<Json<"saveMcpServer", 200>>("PUT", `/mcp/${name}${search}`, config)
    },
    renameMcpServer: (name: string, to: string) => request<Json<"renameMcpServer", 200>>("POST", `/mcp/${name}/rename`, { to }),
    removeMcpServer: (name: string) => request<void>("DELETE", `/mcp/${name}`),
    connectMcpServer: (name: string) => request<Json<"connectMcpServer", 200>>("POST", `/mcp/${name}/connect`),
    disconnectMcpServer: (name: string) => request<Json<"disconnectMcpServer", 200>>("POST", `/mcp/${name}/disconnect`),
    signInMcpServer: (name: string) => request<Json<"signInMcpServer", 200>>("POST", `/mcp/${name}/signin`),
    signOutMcpServer: (name: string) => request<Json<"signOutMcpServer", 200>>("DELETE", `/mcp/${name}/signin`),
    setMcpServerEnabled: (name: string, enabled: boolean) => request<Json<"setMcpServerEnabled", 200>>("PUT", `/mcp/${name}/enabled`, { enabled }),
    todos: (id: string) => request<Json<"listTodos", 200>>("GET", `/sessions/${id}/todos`),
    tasks: (id: string) => request<Json<"listTasks", 200>>("GET", `/sessions/${id}/tasks`),
    stopTask: (id: string) => request<Json<"abortTask", 200>>("POST", `/tasks/${id}/abort`),
  }
}

export type Client = ReturnType<typeof createClient>
