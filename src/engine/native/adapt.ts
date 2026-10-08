// Native engine shapes to the shapes the UI was built on. Dies at M4 when the UI adopts native types.
import type { AssistantMessage, Message, Part, ToolPart } from "../shapes"
import type { ModelInfo, ProviderInfo } from "../store"
import type { components } from "./types"

type NativeMessage = components["schemas"]["Message"]
type NativePartRow = components["schemas"]["PartRow"]
type NativeProvider = components["schemas"]["ProviderStatus"]
type NativeModel = components["schemas"]["Model"]
export type NativeMessageWithParts = components["schemas"]["MessageWithParts"]

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
  if (message.status === "error")
    assistant.error = { name: "UnknownError", data: { message: message.error ?? "The turn failed" } }
  if (message.status === "aborted") assistant.error = { name: "MessageAbortedError", data: { message: "Interrupted" } }
  // The turn stopped itself at a limit: an interruption with its reason, not a failure.
  if (message.status === "paused")
    assistant.error = { name: "MessageAbortedError", data: { message: message.error ?? "Paused" } }
  if (message.status === "done") assistant.finish = "stop"
  applyMessageEnding(assistant, message)
  if (message.summary) assistant.summary = true

  return assistant
}

function applyMessageEnding(assistant: AssistantMessage, message: NativeMessage) {
  // A finished reply that did not end on its own says how (`ending`), and `error` says it in words.
  if (message.status === "done" && message.ending === "length") {
    assistant.finish = "length"
    assistant.error = {
      name: "MessageOutputLengthError",
      data: { message: message.error ?? "The reply stopped at the output limit." },
    }
  }
  if (message.status === "done" && message.ending === "refused") {
    assistant.finish = "content-filter"
    assistant.error = {
      name: "UnknownError",
      data: { message: message.error ?? "The provider's safety filter ended the reply." },
    }
  }
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
      const source = row.path
        ? { source: { type: "file" as const, path: row.path, text: { value, start: 0, end: value.length } } }
        : {}
      return { ...base, type: "file", mime: row.mime, filename: row.name, url: row.url, ...source }
    }
    case "tool_call":
      return { ...base, type: "tool", callID: row.callId, tool: row.name, state: toolState(row) }
    case "compaction":
      return { ...base, type: "compaction", auto: row.auto }
    // Delivered by the engine, not typed by the user: kept out of the user's bubble and the composer history.
    case "task_result":
      return {
        ...base,
        type: "text",
        text: `Background task "${row.description}" ${row.outcome}:\n\n${row.text}`,
        synthetic: true,
      }
    // The engine's own prompt to a working orchestrator: shown, but marked as Drift's, never the user's goal.
    case "nudge":
      return { ...base, type: "text", text: row.text, metadata: { generated: true } }
    case "context":
      return { ...base, type: "plugin", plugin: row.plugin, text: row.text }
    // Rendered as an Answered row; the text is what the model read.
    case "clarification": {
      const items = row.items.map((item) => ({ header: item.header, question: item.question, answers: item.answers }))
      const text = items.map((item) => `${item.question}\nAnswer: ${item.answers.join(", ")}`).join("\n\n")
      return {
        ...base,
        type: "text",
        text,
        metadata: { driftClarification: { version: 1, requestID: row.requestId, items } },
      }
    }
    // Saved by another build or imported: not shown, but its stored text rides along for export.
    case "unknown":
      return {
        ...base,
        type: "text",
        text: "",
        synthetic: true,
        ignored: true,
        metadata: { driftUnknownPart: row.raw },
      }
  }
}

/** The native file tools, whose `path` input the UI reads as `filePath`. */
const FILE_TOOLS = new Set(["read", "edit", "write"])

/** Native tool names for files and patches, as the UI's tool rows, file actions and citations read them. */
export function adaptToolFields(tool: string, rawInput: Record<string, unknown>, rawMetadata: Record<string, unknown>) {
  const input = { ...rawInput }
  const metadata = { ...rawMetadata }
  // `changes` is undo's record; `fileChanges` is the tool's per-file diff for display.
  const changes = Array.isArray(rawMetadata.fileChanges) ? rawMetadata.fileChanges : []
  const written = Array.isArray(rawMetadata.files)
    ? rawMetadata.files.find((file): file is string => typeof file === "string")
    : undefined
  if (FILE_TOOLS.has(tool) && typeof input.path === "string" && input.filePath === undefined)
    input.filePath = written ?? input.path
  if (tool === "apply_patch") {
    if (typeof input.patch === "string" && input.patchText === undefined) input.patchText = input.patch
    // The engine keeps `files` as the paths it wrote; the rows want one change record per file, and one diff for a single file.
    if (changes.length) metadata.files = changes
    const only = changes.length === 1 ? (changes[0] as { patch?: unknown }).patch : undefined
    if (typeof only === "string" && metadata.diff === undefined) metadata.diff = only
  }
  return { input, metadata }
}

function toolState(row: Extract<NativePartRow, { type: "tool_call" }>): ToolPart["state"] {
  const rawInput = (row.input && typeof row.input === "object" ? row.input : { value: row.input }) as Record<
    string,
    unknown
  >
  const { input, metadata } = adaptToolFields(row.name, rawInput, (row.metadata ?? {}) as Record<string, unknown>)
  // A call denied or refused before it ran has no start; 0 would read as a run since 1970.
  const start = (row.startedAt ?? undefined) as number
  const end = row.finishedAt ?? start
  switch (row.status) {
    case "pending":
      return { status: "pending", input, raw: "" }
    case "running":
      return { status: "running", input, title: row.title ?? undefined, metadata, time: { start } }
    case "done":
      return {
        status: "completed",
        input,
        output: row.output ?? "",
        title: row.title ?? row.name,
        metadata,
        time: { start, end },
      }
    case "error":
    case "denied":
      return { status: "error", input, error: row.output ?? "Failed", metadata, time: { start, end } }
  }
}

export function adaptProvider(provider: NativeProvider): ProviderInfo {
  const models = Object.fromEntries(
    Object.values(provider.models).map((model) => [model.id, adaptModel(provider.id, model)]),
  )
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
      input: { text: true, audio: false, image: model.attachment, video: false, pdf: model.pdf ?? false },
      output: { text: true, audio: false, image: false, video: false, pdf: false },
    },
    cost: {
      input: cost.input ?? 0,
      output: cost.output ?? 0,
      cache: { read: cost.cache_read ?? 0, write: cost.cache_write ?? 0 },
    },
    limit: { context: limit.context ?? 0, output: limit.output ?? 0, ...(limit.input ? { input: limit.input } : {}) },
    status: "active",
    options: {},
    headers: {},
    variants: Object.fromEntries((model.variants ?? []).map((variant) => [variant.name, variant])),
  } as ModelInfo
}
