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
    // Spawned threads are top-level rows that link back; only subagents nest under a parent.
    parentID: session.visibility === "hidden" ? session.parentId : undefined,
    spawnedFrom: session.visibility === "sibling" ? session.parentId : undefined,
    title: session.title,
    version: engineVersion,
    time: {
      created: session.createdAt,
      updated: session.updatedAt,
      ...(session.archivedAt ? { archived: session.archivedAt } : {}),
    },
    ...(session.model ? { model: { providerID: session.model.provider, id: session.model.model } } : {}),
    ...(session.revert ? { revert: { messageID: session.revert.messageId } } : {}),
    agent: session.agent,
    variant: session.variant ?? null,
  } as Session
}

/** The engine marks every message with its agent; this only fills the field its schema leaves optional. */
const defaultAgent = "build"

export function adaptMessage(message: NativeMessage, directory: string): Message {
  const model = message.model ?? { provider: "", model: "" }
  const agent = message.agent ?? defaultAgent
  if (message.role === "user") {
    return {
      id: message.id,
      sessionID: message.sessionId,
      role: "user",
      time: { created: message.createdAt },
      agent,
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
    mode: agent,
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
  // The turn stopped itself at a limit: an interruption with its reason, not a failure.
  if (message.status === "paused") assistant.error = { name: "MessageAbortedError", data: { message: message.error ?? "Paused" } }
  if (message.status === "done") assistant.finish = "stop"
  // A finished reply that did not end on its own says how (`ending`), and `error` says it in words.
  if (message.status === "done" && message.ending === "length") {
    assistant.finish = "length"
    assistant.error = { name: "MessageOutputLengthError", data: { message: message.error ?? "The reply stopped at the output limit." } }
  }
  if (message.status === "done" && message.ending === "refused") {
    assistant.finish = "content-filter"
    assistant.error = { name: "UnknownError", data: { message: message.error ?? "The provider's safety filter ended the reply." } }
  }
  if (message.summary) assistant.summary = true
  return assistant
}

export function adaptPart(row: NativePartRow): Part {
  const base = { id: row.id, sessionID: row.sessionId, messageID: row.messageId }
  switch (row.type) {
    case "text":
      return { ...base, type: "text", text: row.text }
    case "reasoning":
      return { ...base, type: "reasoning", text: row.text, time: { start: 0 } }
    case "file": {
      // A mention keeps the workspace file it was read from, so its chip can open that file.
      const value = `@${row.path ?? ""}`
      const source = row.path ? { source: { type: "file" as const, path: row.path, text: { value, start: 0, end: value.length } } } : {}
      return { ...base, type: "file", mime: row.mime, filename: row.name, url: row.url, ...source }
    }
    case "tool_call":
      return { ...base, type: "tool", callID: row.callId, tool: row.name, state: toolState(row) }
    case "compaction":
      return { ...base, type: "compaction", auto: row.auto }
    // Delivered by the engine, not typed by the user: kept out of the user's bubble and the composer history.
    case "task_result":
      return { ...base, type: "text", text: `Background task "${row.description}" ${row.outcome}:\n\n${row.text}`, synthetic: true }
    // Rendered as an Answered row; the text is what the model read.
    case "clarification": {
      const items = row.items.map((item) => ({ header: item.header, question: item.question, answers: item.answers }))
      const text = items.map((item) => `${item.question}\nAnswer: ${item.answers.join(", ")}`).join("\n\n")
      return { ...base, type: "text", text, metadata: { driftClarification: { version: 1, requestID: row.requestId, items } } }
    }
  }
}

function toolState(row: Extract<NativePartRow, { type: "tool_call" }>): ToolPart["state"] {
  const input = (row.input && typeof row.input === "object" ? row.input : { value: row.input }) as Record<string, unknown>
  const metadata = (row.metadata ?? {}) as Record<string, unknown>
  // A call denied or refused before it ran has no start; 0 would read as a run since 1970.
  const start = (row.startedAt ?? undefined) as number
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
    limit: { context: limit.context ?? 0, output: limit.output ?? 0, ...(limit.input ? { input: limit.input } : {}) },
    status: "active",
    options: {},
    headers: {},
    variants: Object.fromEntries((model.variants ?? []).map((variant) => [variant.name, variant])),
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
    case "message.removed":
      return { type: "message.removed", properties: { sessionID: event.sessionId, messageID: event.messageId } }
    case "session.retry":
      return {
        type: "session.status",
        properties: { sessionID: event.sessionId, status: { type: "retry", attempt: event.attempt, message: event.message, next: event.nextAt } },
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
        properties: { sessionID: event.sessionId, messageID: event.messageId, partID: event.partId, field: "text", delta: event.delta, offset: event.offset },
      } as unknown as Event
    case "permission.asked":
      return { type: "permission.updated", properties: adaptPermission(event.request, "") }
    case "permission.replied":
      return { type: "permission.replied", properties: { sessionID: event.sessionId, permissionID: event.requestId, response: event.decision } }
    case "session.deleted":
      return { type: "session.deleted", properties: { info: { id: event.sessionId } as Session } }
    case "todo.updated":
      return { type: "todo.updated", properties: { sessionID: event.sessionId, todos: adaptTodos(event.todos) } }
    case "question.asked":
      return { type: "question.asked", properties: adaptQuestion(event.request) } as unknown as Event
    case "question.replied":
      return { type: "question.replied", properties: { sessionID: event.sessionId, requestID: event.requestId } } as unknown as Event
    case "catalog.updated":
    case "mcp.updated":
    case "mcp.removed":
    case "workspace.created":
    case "task.updated":
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
    async: request.async ?? false,
    tool: { messageID: request.messageId, callID: request.callId },
  }
}

export type { NativeRequest }
