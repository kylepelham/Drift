// Native engine shapes to the shapes the UI was built on. Dies at M4 when the UI adopts native types.
import type { AssistantMessage, Event, Message, Part, Permission, Session, ToolPart } from "@opencode-ai/sdk/client"
import type { ModelInfo, ProviderInfo, QuestionRequest } from "../store"
import type { components } from "./types"

type NativeSession = components["schemas"]["Session"]
type NativeMessage = components["schemas"]["Message"]
type NativePartRow = components["schemas"]["PartRow"]
type NativeEvent = components["schemas"]["Event"]
type NativeRequest = components["schemas"]["PermissionRequest"]
type NativeQuestion = components["schemas"]["QuestionRequest"]
type NativeProvider = components["schemas"]["ProviderStatus"]
type NativeModel = components["schemas"]["Model"]
export type NativeMessageWithParts = components["schemas"]["MessageWithParts"]

/** Workspace ids map to directories because the UI keys everything by directory. */
export type WorkspaceIndex = { path(id: string): string | undefined; id(path: string): string | undefined }

const engineVersion = "drift"

export function adaptSession(session: NativeSession, workspaces: WorkspaceIndex): Session {
  return {
    id: session.id,
    projectID: session.workspaceId,
    directory: workspaces.path(session.workspaceId) ?? session.workspaceId,
    parentID: session.parentId,
    title: session.title,
    version: engineVersion,
    time: {
      created: session.createdAt,
      updated: session.updatedAt,
      ...(session.archivedAt ? { archived: session.archivedAt } : {}),
    },
    ...(session.model ? { model: { providerID: session.model.provider, id: session.model.model } } : {}),
  } as Session
}

export function adaptMessage(message: NativeMessage, directory: string): Message {
  const model = message.model ?? { provider: "", model: "" }
  if (message.role === "user") {
    return {
      id: message.id,
      sessionID: message.sessionId,
      role: "user",
      time: { created: message.createdAt },
      agent: "build",
      model: { providerID: model.provider, modelID: model.model },
    }
  }
  const assistant: AssistantMessage = {
    id: message.id,
    sessionID: message.sessionId,
    role: "assistant",
    time: { created: message.createdAt, ...(message.finishedAt ? { completed: message.finishedAt } : {}) },
    parentID: "",
    modelID: model.model,
    providerID: model.provider,
    mode: "build",
    path: { cwd: directory, root: directory },
    cost: message.cost,
    tokens: {
      input: message.usage.input,
      output: message.usage.output,
      reasoning: 0,
      cache: { read: message.usage.cacheRead, write: message.usage.cacheWrite },
    },
  }
  if (message.status === "error") assistant.error = { name: "UnknownError", data: { message: message.error ?? "The turn failed" } }
  if (message.status === "aborted") assistant.error = { name: "MessageAbortedError", data: { message: "Interrupted" } }
  if (message.status === "done") assistant.finish = "stop"
  return assistant
}

export function adaptPart(row: NativePartRow): Part {
  const base = { id: row.id, sessionID: row.sessionId, messageID: row.messageId }
  switch (row.type) {
    case "text":
      return { ...base, type: "text", text: row.text }
    case "reasoning":
      return { ...base, type: "reasoning", text: row.text, time: { start: 0 } }
    case "file":
      return { ...base, type: "file", mime: row.mime, filename: row.name, url: row.url }
    case "tool_call":
      return { ...base, type: "tool", callID: row.callId, tool: row.name, state: toolState(row) }
  }
}

function toolState(row: Extract<NativePartRow, { type: "tool_call" }>): ToolPart["state"] {
  const input = (row.input && typeof row.input === "object" ? row.input : { value: row.input }) as Record<string, unknown>
  const metadata = (row.metadata ?? {}) as Record<string, unknown>
  const start = row.startedAt ?? 0
  const end = row.finishedAt ?? start
  switch (row.status) {
    case "pending":
      return { status: "pending", input, raw: "" }
    case "running":
      return { status: "running", input, title: row.title ?? undefined, metadata, time: { start } }
    case "done":
      return { status: "completed", input, output: row.output ?? "", title: row.title ?? row.name, metadata, time: { start, end } }
    case "error":
    case "denied":
      return { status: "error", input, error: row.output ?? "Failed", metadata, time: { start, end } }
  }
}

export function adaptPermission(request: NativeRequest, directory: string): Permission {
  return {
    id: request.id,
    type: request.kind,
    pattern: [request.pattern],
    sessionID: request.sessionId,
    messageID: request.messageId,
    callID: request.callId ?? undefined,
    title: request.title,
    metadata: { directory, tool: request.tool },
    time: { created: request.createdAt },
  }
}

export function adaptProvider(provider: NativeProvider): ProviderInfo {
  const models = Object.fromEntries(Object.values(provider.models).map((model) => [model.id, adaptModel(provider.id, model)]))
  return { id: provider.id, name: provider.name, models }
}

function adaptModel(providerID: string, model: NativeModel): ModelInfo {
  const cost: Partial<NonNullable<NativeModel["cost"]>> = model.cost ?? {}
  const limit: Partial<NonNullable<NativeModel["limit"]>> = model.limit ?? {}
  return {
    id: model.id,
    providerID,
    api: { id: model.id, url: "", npm: "" },
    name: model.name,
    family: model.family,
    release_date: model.release_date,
    capabilities: {
      temperature: model.temperature,
      reasoning: model.reasoning,
      attachment: model.attachment,
      toolcall: true,
      input: { text: true, audio: false, image: model.attachment, video: false, pdf: model.attachment },
      output: { text: true, audio: false, image: false, video: false, pdf: false },
    },
    cost: { input: cost.input ?? 0, output: cost.output ?? 0, cache: { read: cost.cache_read ?? 0, write: cost.cache_write ?? 0 } },
    limit: { context: limit.context ?? 0, output: limit.output ?? 0 },
    status: "active",
    options: {},
    headers: {},
  } as ModelInfo
}

/** One native event becomes the legacy event the existing reducer already understands. */
export function adaptEvent(event: NativeEvent, workspaces: WorkspaceIndex): Event | undefined {
  switch (event.type) {
    case "session.created":
    case "session.updated":
      return { type: "session.updated", properties: { info: adaptSession(event.session, workspaces) } }
    case "session.status":
      return {
        type: "session.status",
        properties: { sessionID: event.sessionId, status: event.status === "running" ? { type: "busy" } : { type: "idle" } },
      }
    case "message.created":
    case "message.updated":
      return { type: "message.updated", properties: { info: adaptMessage(event.message, "") } }
    case "part.created":
    case "part.updated":
      return { type: "message.part.updated", properties: { part: adaptPart(event.part) } }
    case "part.delta":
      return {
        type: "message.part.delta",
        properties: { sessionID: event.sessionId, messageID: event.messageId, partID: event.partId, field: "text", delta: event.delta },
      } as unknown as Event
    case "permission.asked":
      return { type: "permission.updated", properties: adaptPermission(event.request, "") }
    case "permission.replied":
      return { type: "permission.replied", properties: { sessionID: event.sessionId, permissionID: event.requestId, response: event.decision } }
    case "todo.updated":
      return { type: "todo.updated", properties: { sessionID: event.sessionId, todos: adaptTodos(event.todos) } }
    case "question.asked":
      return { type: "question.asked", properties: adaptQuestion(event.request) } as unknown as Event
    case "question.replied":
      return { type: "question.replied", properties: { sessionID: event.sessionId, requestID: event.requestId } } as unknown as Event
    case "workspace.created":
      return undefined
  }
}

export function adaptTodos(todos: components["schemas"]["Todo"][]) {
  return todos.map((todo, index) => ({ id: String(index), content: todo.content, status: todo.status, priority: todo.priority ?? "medium" }))
}

export function adaptQuestion(request: NativeQuestion): QuestionRequest {
  return {
    id: request.id,
    sessionID: request.sessionId,
    questions: request.questions.map((q) => ({ question: q.question, header: q.header ?? "", options: (q.options ?? []).map((o) => ({ label: o.label, description: o.description ?? "" })), multiple: q.multiple ?? false, custom: q.custom ?? true })),
    tool: { messageID: request.messageId, callID: request.callId },
  }
}

export type { NativeRequest }
