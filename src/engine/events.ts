import type { Event, Message, Part, Permission, Session } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { clearQuestionDraft } from "../state/question-drafts"
import { errorText } from "./error"
import { dropSessionState, putSession, recordLink, spawnLink, type EngineState, type Notice, type QuestionRequest } from "./store"

type Set = SetStoreFunction<EngineState>

export function reduce(set: Set, event: Event, directory?: string) {
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

function upsertSession(set: Set, info: Session) {
  putSession(set, info)
}

function dropSession(set: Set, info: Session) {
  set(
    produce((s) => {
      dropSessionState(s, info.id)
    }),
  )
}

function moveSession(
  set: Set,
  moved: {
    sessionID: string
    projectID?: string
    location: { directory: string; workspaceID?: string }
    subdirectory?: string
    timestamp: number
  },
) {
  set(
    produce((state) => {
      const session = state.sessions[moved.sessionID]
      if (!session) return
      session.directory = moved.location.directory
      if (moved.projectID) session.projectID = moved.projectID
      session.time.updated = moved.timestamp
    }),
  )
}

function recordError(set: Set, sessionID?: string, error?: { name: string; data?: unknown }) {
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
    produce((s) => {
      s.status[sessionID] = { type: "idle" }
      if (s.activity[sessionID]) s.activity[sessionID].current = undefined
      if (error?.name === "MessageAbortedError") return
      s.errors[sessionID] = message
    }),
  )
}

function clearError(set: Set, sessionID: string) {
  set(
    produce((state) => {
      delete state.errors[sessionID]
    }),
  )
}

function upsertMessage(set: Set, info: Message) {
  set(
    produce((s) => {
      const list = s.loaded[info.sessionID] ? s.transcripts[info.sessionID] : undefined
      if (!list) return
      const index = list.findIndex((entry) => entry.info.id === info.id)
      if (index >= 0) list[index].info = info
      else list.push({ info, parts: [] })
    }),
  )
}

function dropMessage(set: Set, sessionID: string, messageID: string) {
  set(
    produce((s) => {
      const list = s.transcripts[sessionID]
      if (list) s.transcripts[sessionID] = list.filter((entry) => entry.info.id !== messageID)
    }),
  )
}

function upsertPart(set: Set, part: Part) {
  const link = spawnLink(part)
  if (link) recordLink(link)
  set(
    produce((s) => {
      if (link) s.links[link.child] = link.parent
      if (part.type === "tool") trackActivity(s, part)
      const entry = s.transcripts[part.sessionID]?.find((item) => item.info.id === part.messageID)
      if (!entry) return
      const index = entry.parts.findIndex((existing) => existing.id === part.id)
      if (index >= 0) entry.parts[index] = part
      else entry.parts.push(part)
    }),
  )
}

function appendPartDelta(
  set: Set,
  ref: { sessionID: string; messageID: string; partID: string; field: string; delta: string },
) {
  set(
    produce((s) => {
      const entry = s.transcripts[ref.sessionID]?.find((item) => item.info.id === ref.messageID)
      const part = entry?.parts.find((item) => item.id === ref.partID)
      if (!part) return
      const record = part as unknown as Record<string, unknown>
      const current = record[ref.field]
      if (typeof current === "string") record[ref.field] = current + ref.delta
    }),
  )
}

function trackActivity(s: EngineState, part: Part & { type: "tool" }) {
  const entry = s.activity[part.sessionID] ?? { tools: 0, lastPartId: "" }
  if (entry.lastPartId !== part.id) {
    entry.tools += 1
    entry.lastPartId = part.id
  }
  entry.current = part.state.status === "completed" || part.state.status === "error" ? undefined : part.tool
  s.activity[part.sessionID] = entry
}

function dropPart(set: Set, ref: { sessionID: string; messageID: string; partID: string }) {
  set(
    produce((s) => {
      const entry = s.transcripts[ref.sessionID]?.find((item) => item.info.id === ref.messageID)
      if (entry) entry.parts = entry.parts.filter((part) => part.id !== ref.partID)
    }),
  )
}

function addQuestion(set: Set, question: QuestionRequest) {
  set(
    produce((s) => {
      const list = s.questions[question.sessionID] ?? []
      if (!list.some((existing) => existing.id === question.id)) list.push(question)
      s.questions[question.sessionID] = list
    }),
  )
}

function dropQuestion(set: Set, sessionID: string, requestID: string) {
  clearQuestionDraft(requestID)
  set(
    produce((s) => {
      const list = s.questions[sessionID]
      if (list) s.questions[sessionID] = list.filter((question) => question.id !== requestID)
    }),
  )
}

function addPermission(set: Set, permission: Permission) {
  set(
    produce((s) => {
      const list = s.permissions[permission.sessionID] ?? []
      if (!list.some((existing) => existing.id === permission.id)) list.push(permission)
      s.permissions[permission.sessionID] = list
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

function dropPermission(set: Set, sessionID: string, permissionID: string) {
  set(
    produce((s) => {
      const list = s.permissions[sessionID]
      if (list) s.permissions[sessionID] = list.filter((permission) => permission.id !== permissionID)
    }),
  )
}

let noticeSequence = 0

function noticeVariant(value: unknown): Notice["variant"] {
  return value === "success" || value === "warning" || value === "error" ? value : "info"
}

export function pushNotice(set: Set, notice: Notice) {
  set(
    produce((state) => {
      state.notices = [
        ...state.notices.filter(
          (item) =>
            item.id !== notice.id &&
            (item.title !== notice.title || item.message !== notice.message || item.variant !== notice.variant),
        ),
        notice,
      ].slice(-6)
    }),
  )
}
