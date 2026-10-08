// Typed access to the native engine. Request and response shapes come from the generated types.
import type { components, operations } from "./types"

export type Target = { url: string; token: string }

/** The engine's `MAX_REQUEST_BYTES`: a larger request is cut off mid-upload, which a browser reports only as a failed fetch. */
export const maxRequestBytes = 64 * 1024 * 1024
export type Workspace = components["schemas"]["Workspace"]
export type PluginInfo = components["schemas"]["PluginInfo"]
export type SkillPack = components["schemas"]["Pack"]
export type UserSkill = components["schemas"]["UserSkill"]
export type NewWorkspace = components["schemas"]["NewWorkspace"]
export type Health = components["schemas"]["Health"]
export type Frame = components["schemas"]["Frame"]
export type Envelope = components["schemas"]["Envelope"]
export type Incoming = components["schemas"]["Incoming"]
export type Event = components["schemas"]["Event"]
export type Session = components["schemas"]["Session"]
export type MessageWithParts = components["schemas"]["MessageWithParts"]
export type Prompt = components["schemas"]["Prompt"]
export type Receipt = components["schemas"]["Receipt"]
export type PermissionRequest = components["schemas"]["PermissionRequest"]
export type PermissionRule = components["schemas"]["Rule"]
export type PermissionGrant = components["schemas"]["Grant"]
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

type Json<Op extends keyof operations, Status extends number> =
  operations[Op]["responses"] extends Record<Status, { content: { "application/json": infer Body } }> ? Body : never

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

/** The active workspace a stdio MCP server connects in, as a query string. */
const inWorkspace = (workspace?: string) => (workspace ? `?${new URLSearchParams({ workspace })}` : "")

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
    for (const [key, value] of Object.entries(params))
      if (value !== undefined && value !== null) search.set(key, String(value))
    const text = search.toString()
    return text ? `?${text}` : ""
  }
  return {
    health: () => request<Json<"health", 200>>("GET", "/health"),
    workspaces: () => request<Json<"listWorkspaces", 200>>("GET", "/workspaces"),
    workspaceConfig: (id: string) => request<Json<"workspaceConfig", 200>>("GET", `/workspaces/${id}/config`),
    runCommand: (id: string, name: string, args: string) =>
      request<Json<"runCommand", 202>>("POST", `/sessions/${id}/command`, { name, arguments: args }),
    createWorkspace: (body: NewWorkspace) => request<Json<"createWorkspace", 201>>("POST", "/workspaces", body),
    sessions: (params: operations["listSessions"]["parameters"]["query"] = {}) =>
      request<Json<"listSessions", 200>>("GET", `/sessions${query({ ...params })}`),
    createSession: (body: components["schemas"]["NewSessionBody"]) =>
      request<Json<"createSession", 201>>("POST", "/sessions", body),
    spawnThread: (id: string, instruction: string) =>
      request<Json<"spawnThread", 201>>("POST", `/sessions/${id}/spawn`, { instruction }),
    forkSession: (id: string, atMessage?: string) =>
      request<Json<"forkSession", 201>>("POST", `/sessions/${id}/fork`, { atMessage }),
    moveSession: (id: string, workspaceId: string) =>
      request<Json<"moveSession", 200>>("POST", `/sessions/${id}/move`, { workspaceId }),
    compactSession: (id: string) => request<void>("POST", `/sessions/${id}/compact`),
    switchRetryModel: (id: string, model: components["schemas"]["ModelRef"], variant: string | null) =>
      request<void>("POST", `/sessions/${id}/retry`, { model, variant }),
    revertSession: (id: string, messageId: string, keepFiles = false) =>
      request<Json<"revertSession", 200>>("POST", `/sessions/${id}/revert`, { messageId, keepFiles }),
    unrevertSession: (id: string) => request<Json<"unrevertSession", 200>>("POST", `/sessions/${id}/unrevert`),
    settings: () => request<Json<"getSettings", 200>>("GET", "/settings"),
    putSettings: (body: components["schemas"]["EngineSettingsInput"]) =>
      request<Json<"putSettings", 200>>("PUT", "/settings", body),
    fetchRegistry: (source: string) => request<unknown>("GET", `/registries/fetch?${new URLSearchParams({ source })}`),
    basePrompts: () => request<Json<"listBasePrompts", 200>>("GET", "/prompts"),
    plugins: () => request<Json<"listPlugins", 200>>("GET", "/plugins"),
    reloadPlugins: () => request<Json<"reloadPlugins", 200>>("POST", "/plugins/reload"),
    setPluginEnabled: (path: string, enabled: boolean) =>
      request<Json<"setPluginEnabled", 200>>("PUT", "/plugins/enabled", { path, enabled }),
    installPlugin: (body: components["schemas"]["Install"]) =>
      request<Json<"installPlugin", 200>>("POST", "/plugins/install", body),
    removePlugin: (path: string) =>
      request<Json<"removePlugin", 200>>("DELETE", `/plugins?${new URLSearchParams({ path })}`),
    skills: (workspaceId?: string) =>
      request<Json<"listSkills", 200>>(
        "GET",
        `/skills${workspaceId ? `?${new URLSearchParams({ workspace: workspaceId })}` : ""}`,
      ),
    setSkillEnabled: (path: string, enabled: boolean, workspace?: string) =>
      request<Json<"setSkillEnabled", 200>>("PUT", "/skills/enabled", { path, enabled, workspace }),
    skillPacks: () => request<Json<"listSkillPacks", 200>>("GET", "/skills/packs"),
    installSkillPack: (body: components["schemas"]["InstallPack"]) =>
      request<Json<"installSkillPack", 200>>("POST", "/skills/packs", body),
    removeSkillPack: (id: string) =>
      request<Json<"removeSkillPack", 200>>("DELETE", `/skills/packs?${new URLSearchParams({ id })}`),
    configurePlugin: (path: string, config: unknown) =>
      request<Json<"configurePlugin", 200>>("PUT", "/plugins/config", { path, config }),
    tools: (workspaceId?: string) =>
      request<Json<"listTools", 200>>(
        "GET",
        `/tools${workspaceId ? `?${new URLSearchParams({ workspace: workspaceId })}` : ""}`,
      ),
    saveBasePrompt: (id: string, text: string) =>
      request<Json<"saveBasePrompt", 200>>("PUT", `/prompts/${id}`, { text }),
    resetBasePrompt: (id: string) => request<Json<"resetBasePrompt", 200>>("DELETE", `/prompts/${id}`),
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
      request<Json<"findFiles", 200>>(
        "GET",
        `/workspaces/${workspaceId}/files${query({ query: text })}`,
        undefined,
        signal,
      ),
    abort: (id: string) => request<Json<"abortTurn", 200>>("POST", `/sessions/${id}/abort`),
    providers: () => request<Json<"listProviders", 200>>("GET", "/providers"),
    setProviderKey: (id: string, key: string) => request<void>("PUT", `/providers/${id}/key`, { key }),
    removeProviderCredentials: (id: string) => request<void>("DELETE", `/providers/${id}/credentials`),
    startOAuth: (id: string, mode: "max" | "console" | "chatgpt" | "supergrok") =>
      request<Json<"startOAuth", 200>>("POST", `/providers/${id}/oauth`, { mode }),
    finishOAuth: (id: string, input: string, state?: string) =>
      request<void>("POST", `/providers/${id}/oauth/callback`, { input, state }),
    permissions: () => request<Json<"listPermissions", 200>>("GET", "/permissions"),
    replyPermission: (id: string, body: ReplyBody) => request<void>("POST", `/permissions/${id}/reply`, body),
    permissionRules: () => request<Json<"listPermissionRules", 200>>("GET", "/permission-rules"),
    savePermissionRules: (rules: PermissionRule[]) =>
      request<Json<"savePermissionRules", 200>>("PUT", "/permission-rules", rules),
    permissionGrants: (workspaceId: string) =>
      request<Json<"listPermissionGrants", 200>>("GET", `/workspaces/${workspaceId}/permission-grants`),
    revokePermissionGrant: (workspaceId: string, grant: PermissionGrant) =>
      request<void>("POST", `/workspaces/${workspaceId}/permission-grants/revoke`, grant),
    revokePermissionGrants: (workspaceId: string) =>
      request<void>("DELETE", `/workspaces/${workspaceId}/permission-grants`),
    questions: () => request<Json<"listQuestions", 200>>("GET", "/questions"),
    answerQuestion: (id: string, answers: string[][]) => request<void>("POST", `/questions/${id}/reply`, { answers }),
    rejectQuestion: (id: string) => request<void>("POST", `/questions/${id}/reject`),
    mcpServers: () => request<Json<"listMcpServers", 200>>("GET", "/mcp"),
    saveMcpServer: (
      name: string,
      config: McpServerConfig,
      options: { create?: boolean; readOnlyTrusted?: boolean; workspace?: string } = {},
    ) => {
      const query = new URLSearchParams()
      if (options.create) query.set("create", "true")
      if (options.readOnlyTrusted !== undefined) query.set("readOnlyTrusted", String(options.readOnlyTrusted))
      if (options.workspace) query.set("workspace", options.workspace)
      const search = query.size ? `?${query}` : ""
      return request<Json<"saveMcpServer", 200>>("PUT", `/mcp/${name}${search}`, config)
    },
    renameMcpServer: (name: string, to: string, workspace?: string) =>
      request<Json<"renameMcpServer", 200>>("POST", `/mcp/${name}/rename${inWorkspace(workspace)}`, { to }),
    removeMcpServer: (name: string) => request<void>("DELETE", `/mcp/${name}`),
    connectMcpServer: (name: string, workspace?: string) =>
      request<Json<"connectMcpServer", 200>>("POST", `/mcp/${name}/connect${inWorkspace(workspace)}`),
    disconnectMcpServer: (name: string, workspace?: string) =>
      request<Json<"disconnectMcpServer", 200>>("POST", `/mcp/${name}/disconnect${inWorkspace(workspace)}`),
    signInMcpServer: (name: string) => request<Json<"signInMcpServer", 200>>("POST", `/mcp/${name}/signin`),
    signOutMcpServer: (name: string) => request<Json<"signOutMcpServer", 200>>("DELETE", `/mcp/${name}/signin`),
    setMcpServerEnabled: (name: string, enabled: boolean, workspace?: string) =>
      request<Json<"setMcpServerEnabled", 200>>("PUT", `/mcp/${name}/enabled${inWorkspace(workspace)}`, { enabled }),
    todos: (id: string) => request<Json<"listTodos", 200>>("GET", `/sessions/${id}/todos`),
    tasks: (id: string) => request<Json<"listTasks", 200>>("GET", `/sessions/${id}/tasks`),
    stopTask: (id: string) => request<Json<"abortTask", 200>>("POST", `/tasks/${id}/abort`),
  }
}

export type Client = ReturnType<typeof createClient>
