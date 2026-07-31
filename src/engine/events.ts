import type { Event, Message, Part, Permission, Session } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { clearQuestionDraft } from "../state/question-drafts"
import {
  clearPermissionAttention,
  clearPermissionAttentionFor,
  observePermission,
  type DriftPermission,
} from "../state/permission-attention"
import { errorText } from "./error"
import {
  bumpAskRevision,
  putSession,
  recordLink,
  spawnLink,
  type EngineState,
  type Notice,
  type QuestionRequest,
} from "./store"

type SetEngineState = SetStoreFunction<EngineState>

export function reduce(set: SetEngineState, event: Event, directory?: string, state?: EngineState) {
  // These events are newer than the generated v1 SDK's Event union.
  const raw = event as { id?: string; type: string; properties: Record<string, unknown> }
  if (raw.type === "question.v2.asked" || raw.type === "question.asked")
    return addQuestion(set, { ...(raw.properties as unknown as QuestionRequest), directory })
  if (
    raw.type === "question.v2.replied" ||
    raw.type === "question.v2.rejected" ||
    raw.type === "question.replied" ||
    raw.type === "question.rejected"
  )
    return dropQuestion(set, raw.properties.sessionID as string, raw.properties.requestID as string, directory)
  if (raw.type === "permission.asked" || raw.type === "permission.v2.asked")
    return addPermission(
      set,
      permissionFromEvent(raw.properties, directory, raw.type === "permission.v2.asked"),
      directory,
      state,
    )
  if (raw.type === "permission.v2.replied" || raw.type === "permission.replied")
    return dropPermission(
      set,
      raw.properties.sessionID as string,
      (raw.properties.requestID ?? raw.properties.permissionID) as string,
      directory,
    )
  if (raw.type === "tui.toast.show")
    return pushNotice(set, {
      id: raw.id ?? `notice-${Date.now()}-${noticeSequence++}`,
      title: typeof raw.properties.title === "string" ? raw.properties.title : undefined,
      message: String(raw.properties.message ?? ""),
      variant: noticeVariant(raw.properties.variant),
      created: Date.now(),
      duration: typeof raw.properties.duration === "number" ? raw.properties.duration : 5000,
    })
  if (raw.type === "message.part.delta")
    return appendPartDelta(set, raw.properties as { sessionID: string; messageID: string; partID: string; field: string; delta: string })
  if (raw.type === "session.compacted") return clearError(set, raw.properties.sessionID as string)
  if (raw.type === "session.next.moved")
    return moveSession(
      set,
      raw.properties as {
        sessionID: string
        projectID?: string
        location: { directory: string; workspaceID?: string }
        subdirectory?: string
        timestamp: number
      },
    )
  switch (event.type) {
    case "session.created":
    case "session.updated":
      return upsertSession(set, event.properties.info)
    case "session.deleted":
      return dropSession(set, event.properties.info)
    case "session.status": {
      const sessionID = event.properties.sessionID
      set(
        produce((draft) => {
          draft.status[sessionID] = event.properties.status
          if (event.properties.status.type === "idle") clearLiveTools(draft, sessionID)
        }),
      )
      if (event.properties.status.type !== "idle") clearError(set, event.properties.sessionID)
      return
    }
    case "session.idle":
      return set(
        produce((draft) => {
          draft.status[event.properties.sessionID] = { type: "idle" }
          clearLiveTools(draft, event.properties.sessionID)
        }),
      )
    case "session.error":
      return recordError(set, event.properties.sessionID, event.properties.error)
    case "message.updated":
      return upsertMessage(set, event.properties.info)
    case "message.removed":
      return dropMessage(set, event.properties.sessionID, event.properties.messageID)
    case "message.part.updated":
      return upsertPart(set, event.properties.part)
    case "message.part.removed":
      return dropPart(set, event.properties)
    case "permission.updated":
      return addPermission(set, event.properties, directory, state)
    case "permission.replied":
      return dropPermission(set, event.properties.sessionID, event.properties.permissionID, directory)
    case "todo.updated":
      return set("todos", event.properties.sessionID, event.properties.todos)
  }
}

function upsertSession(set: SetEngineState, info: Session) {
  putSession(set, info)
}

// Full purge of every per-session slice, in response to the engine reporting a deleted session.
// NOTE: actions.ts forgetSession covers only sessions/transcripts/loaded. See the note there.
function dropSession(set: SetEngineState, info: Session) {
  set(
    produce((draft) => {
      clearPermissionAttentionFor(draft.permissions[info.id] ?? [])
      delete draft.sessions[info.id]
      delete draft.transcripts[info.id]
      delete draft.loaded[info.id]
      delete draft.permissions[info.id]
      delete draft.questions[info.id]
      delete draft.todos[info.id]
      delete draft.status[info.id]
      delete draft.activity[info.id]
      delete draft.errors[info.id]
      clearLiveTools(draft, info.id)
    }),
  )
}

function clearLiveTools(draft: EngineState, sessionID: string) {
  for (const [partID, owner] of Object.entries(draft.liveTools)) if (owner === sessionID) delete draft.liveTools[partID]
}

function moveSession(
  set: SetEngineState,
  moved: {
    sessionID: string
    projectID?: string
    location: { directory: string; workspaceID?: string }
    subdirectory?: string
    timestamp: number
  },
) {
  set(
    produce((draft) => {
      const session = draft.sessions[moved.sessionID]
      if (!session) return
      session.directory = moved.location.directory
      if (moved.projectID) session.projectID = moved.projectID
      session.time.updated = moved.timestamp
    }),
  )
}

function recordError(set: SetEngineState, sessionID?: string, error?: { name: string; data?: unknown }) {
  const message = errorText(error)
  if (!sessionID) {
    pushNotice(set, {
      id: `session-error-${Date.now()}-${noticeSequence++}`,
      title: "Drift error",
      message,
      variant: "error",
      created: Date.now(),
      duration: 8000,
    })
    return
  }
  set(
    produce((draft) => {
      draft.status[sessionID] = { type: "idle" }
      if (draft.activity[sessionID]) draft.activity[sessionID].current = undefined
      clearLiveTools(draft, sessionID)
      if (error?.name === "MessageAbortedError") return
      draft.errors[sessionID] = message
    }),
  )
}

function clearError(set: SetEngineState, sessionID: string) {
  set(
    produce((draft) => {
      delete draft.errors[sessionID]
    }),
  )
}

function upsertMessage(set: SetEngineState, info: Message) {
  set(
    produce((draft) => {
      const list = draft.loaded[info.sessionID] ? draft.transcripts[info.sessionID] : undefined
      if (!list) return
      const index = list.findIndex((entry) => entry.info.id === info.id)
      if (index >= 0) list[index].info = info
      else list.push({ info, parts: [] })
    }),
  )
}

function dropMessage(set: SetEngineState, sessionID: string, messageID: string) {
  set(
    produce((draft) => {
      const list = draft.transcripts[sessionID]
      if (list) draft.transcripts[sessionID] = list.filter((entry) => entry.info.id !== messageID)
    }),
  )
}

function upsertPart(set: SetEngineState, part: Part) {
  const link = spawnLink(part)
  if (link) recordLink(link)
  set(
    produce((draft) => {
      if (link) draft.links[link.child] = link.parent
      if (part.type === "tool") {
        trackActivity(draft, part)
        if (part.state.status === "pending" || part.state.status === "running") draft.liveTools[part.id] = part.sessionID
        else delete draft.liveTools[part.id]
      }
      const entry = draft.transcripts[part.sessionID]?.find((item) => item.info.id === part.messageID)
      if (!entry) return
      const index = entry.parts.findIndex((existing) => existing.id === part.id)
      if (index >= 0) entry.parts[index] = part
      else entry.parts.push(part)
    }),
  )
}

function appendPartDelta(
  set: SetEngineState,
  ref: { sessionID: string; messageID: string; partID: string; field: string; delta: string },
) {
  set(
    produce((draft) => {
      const entry = draft.transcripts[ref.sessionID]?.find((item) => item.info.id === ref.messageID)
      const part = entry?.parts.find((item) => item.id === ref.partID)
      if (!part) return
      const record = part as unknown as Record<string, unknown>
      const current = record[ref.field]
      if (typeof current === "string") record[ref.field] = current + ref.delta
    }),
  )
}

function trackActivity(draft: EngineState, part: Part & { type: "tool" }) {
  const entry = draft.activity[part.sessionID] ?? { tools: 0, lastPartId: "" }
  if (entry.lastPartId !== part.id) {
    entry.tools += 1
    entry.lastPartId = part.id
  }
  entry.current = part.state.status === "completed" || part.state.status === "error" ? undefined : part.tool
  draft.activity[part.sessionID] = entry
}

function dropPart(set: SetEngineState, ref: { sessionID: string; messageID: string; partID: string }) {
  set(
    produce((draft) => {
      const entry = draft.transcripts[ref.sessionID]?.find((item) => item.info.id === ref.messageID)
      if (entry) entry.parts = entry.parts.filter((part) => part.id !== ref.partID)
      delete draft.liveTools[ref.partID]
    }),
  )
}

function addQuestion(set: SetEngineState, question: QuestionRequest) {
  set(
    produce((draft) => {
      bumpAskRevision(draft, "question", question.directory)
      const list = draft.questions[question.sessionID] ?? []
      if (!list.some((existing) => existing.id === question.id)) list.push(question)
      draft.questions[question.sessionID] = list
    }),
  )
}

function dropQuestion(set: SetEngineState, sessionID: string, requestID: string, directory?: string) {
  clearQuestionDraft(requestID)
  set(
    produce((draft) => {
      const list = draft.questions[sessionID]
      const current = list?.find((question) => question.id === requestID)
      bumpAskRevision(draft, "question", current?.directory ?? directory)
      if (list) draft.questions[sessionID] = list.filter((question) => question.id !== requestID)
    }),
  )
}

function addPermission(set: SetEngineState, permission: Permission, directory?: string, state?: EngineState) {
  const resolvedDirectory =
    typeof permission.metadata?.directory === "string" ? permission.metadata.directory : directory
  const entry =
    resolvedDirectory && !permission.metadata?.directory
      ? { ...permission, metadata: { ...permission.metadata, directory: resolvedDirectory } }
      : permission
  observePermission(entry, state)
  set(
    produce((draft) => {
      bumpAskRevision(draft, "permission", resolvedDirectory)
      const list = draft.permissions[entry.sessionID] ?? []
      if (!list.some((existing) => existing.id === entry.id)) list.push(entry)
      draft.permissions[entry.sessionID] = list
    }),
  )
}

function permissionFromEvent(properties: Record<string, unknown>, directory?: string, v2 = false): DriftPermission {
  const source = properties.source as { messageID?: string; callID?: string } | undefined
  const tool = properties.tool as { messageID?: string; callID?: string } | undefined
  const metadata = (properties.metadata as Record<string, unknown> | undefined) ?? {}
  const type = String(properties.permission ?? properties.action ?? "permission")
  const patterns = properties.patterns ?? properties.resources
  const always = v2 ? properties.save : properties.always
  return {
    id: String(properties.id),
    type,
    pattern: Array.isArray(patterns) ? patterns.map(String) : undefined,
    sessionID: String(properties.sessionID),
    messageID: tool?.messageID ?? source?.messageID ?? "",
    callID: tool?.callID ?? source?.callID,
    title: String(metadata.title ?? type),
    metadata: {
      ...metadata,
      ...(Array.isArray(always) ? { always: always.map(String) } : {}),
      ...(directory ? { directory } : {}),
    },
    time: { created: Date.now() },
    ...(v2 ? { driftProtocol: "v2" as const } : {}),
  }
}

function dropPermission(set: SetEngineState, sessionID: string, permissionID: string, directory?: string) {
  clearPermissionAttention(permissionID)
  set(
    produce((draft) => {
      const list = draft.permissions[sessionID]
      const current = list?.find((permission) => permission.id === permissionID)
      const currentDirectory = current?.metadata?.directory
      bumpAskRevision(draft, "permission", typeof currentDirectory === "string" ? currentDirectory : directory)
      if (list) draft.permissions[sessionID] = list.filter((permission) => permission.id !== permissionID)
    }),
  )
}

let noticeSequence = 0

function noticeVariant(value: unknown): Notice["variant"] {
  return value === "success" || value === "warning" || value === "error" ? value : "info"
}

export function pushNotice(set: SetEngineState, notice: Notice) {
  set(
    produce((draft) => {
      draft.notices = [
        ...draft.notices.filter(
          (item) =>
            item.id !== notice.id &&
            (item.title !== notice.title || item.message !== notice.message || item.variant !== notice.variant),
        ),
        notice,
      ].slice(-6)
    }),
  )
}
