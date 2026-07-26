import type { Event, Message, Part, Permission, Session } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { clearQuestionDraft } from "../state/question-drafts"
import { errorText } from "./error"
import { putSession, recordLink, spawnLink, type EngineState, type Notice, type QuestionRequest } from "./store"

type SetEngineState = SetStoreFunction<EngineState>

export function reduce(set: SetEngineState, event: Event, directory?: string) {
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
    return dropQuestion(set, raw.properties.sessionID as string, raw.properties.requestID as string)
  if (raw.type === "permission.asked" || raw.type === "permission.v2.asked")
    return addPermission(set, permissionFromEvent(raw.properties, directory))
  if (raw.type === "permission.v2.replied" || raw.type === "permission.replied")
    return dropPermission(
      set,
      raw.properties.sessionID as string,
      (raw.properties.requestID ?? raw.properties.permissionID) as string,
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
    case "session.status":
      set("status", event.properties.sessionID, event.properties.status)
      if (event.properties.status.type !== "idle") clearError(set, event.properties.sessionID)
      return
    case "session.idle":
      return set("status", event.properties.sessionID, { type: "idle" })
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
      return addPermission(set, event.properties)
    case "permission.replied":
      return dropPermission(set, event.properties.sessionID, event.properties.permissionID)
    case "todo.updated":
      return set("todos", event.properties.sessionID, event.properties.todos)
  }
}

function upsertSession(set: SetEngineState, info: Session) {
  putSession(set, info)
}

function dropSession(set: SetEngineState, info: Session) {
  set(
    produce((draft) => {
      delete draft.sessions[info.id]
      delete draft.transcripts[info.id]
      delete draft.loaded[info.id]
      delete draft.permissions[info.id]
      delete draft.questions[info.id]
      delete draft.todos[info.id]
      delete draft.status[info.id]
      delete draft.activity[info.id]
      delete draft.errors[info.id]
    }),
  )
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
      if (part.type === "tool") trackActivity(draft, part)
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
    }),
  )
}

function addQuestion(set: SetEngineState, question: QuestionRequest) {
  set(
    produce((draft) => {
      const list = draft.questions[question.sessionID] ?? []
      if (!list.some((existing) => existing.id === question.id)) list.push(question)
      draft.questions[question.sessionID] = list
    }),
  )
}

function dropQuestion(set: SetEngineState, sessionID: string, requestID: string) {
  clearQuestionDraft(requestID)
  set(
    produce((draft) => {
      const list = draft.questions[sessionID]
      if (list) draft.questions[sessionID] = list.filter((question) => question.id !== requestID)
    }),
  )
}

function addPermission(set: SetEngineState, permission: Permission) {
  set(
    produce((draft) => {
      const list = draft.permissions[permission.sessionID] ?? []
      if (!list.some((existing) => existing.id === permission.id)) list.push(permission)
      draft.permissions[permission.sessionID] = list
    }),
  )
}

function permissionFromEvent(properties: Record<string, unknown>, directory?: string): Permission {
  const source = properties.source as { messageID?: string; callID?: string } | undefined
  const tool = properties.tool as { messageID?: string; callID?: string } | undefined
  const metadata = (properties.metadata as Record<string, unknown> | undefined) ?? {}
  const type = String(properties.permission ?? properties.action ?? "permission")
  const patterns = properties.patterns ?? properties.resources
  return {
    id: String(properties.id),
    type,
    pattern: Array.isArray(patterns) ? patterns.map(String) : undefined,
    sessionID: String(properties.sessionID),
    messageID: tool?.messageID ?? source?.messageID ?? "",
    callID: tool?.callID ?? source?.callID,
    title: String(metadata.title ?? type),
    metadata: directory ? { ...metadata, directory } : metadata,
    time: { created: Date.now() },
  }
}

function dropPermission(set: SetEngineState, sessionID: string, permissionID: string) {
  set(
    produce((draft) => {
      const list = draft.permissions[sessionID]
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
