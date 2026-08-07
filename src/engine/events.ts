import type { Event, Message, Part, Permission, Session, SessionStatus } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { clearQuestionDraft } from "../state/question-drafts"
import {
  clearPermissionAttention,
  clearPermissionAttentionFor,
  observePermission,
  type DriftPermission,
} from "../state/permission-attention"
import { classifyRecoverableError, errorText, type EngineError } from "./error"
import {
  clearRecoverableInterruption,
  interruptionIdentity,
  recordRecoverableInterruption,
} from "../state/recovery"
import { workspaces } from "../state/workspaces"
import {
  bumpAskRevision,
  bumpRevision,
  messageRevisionKey,
  normalizeDir,
  pruneSessionRevisions,
  putSession,
  recordLink,
  revisionAdvanced,
  sessionRevisionKey,
  spawnLink,
  statusRevisionKey,
  type EngineState,
  type ModelRef,
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
  if (raw.type === "session.compacted") {
    const sessionID = raw.properties.sessionID as string
    clearError(set, sessionID)
    clearRecoverableInterruption(sessionID, true)
    return
  }
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
          bumpRevision(draft, statusRevisionKey(sessionID))
          if (event.properties.status.type === "idle") clearLiveTools(draft, sessionID)
        }),
      )
      if (event.properties.status.type !== "idle") {
        clearError(set, event.properties.sessionID)
        clearRecoverableInterruption(event.properties.sessionID, true)
      }
      return
    }
    case "session.idle":
      return set(
        produce((draft) => {
          draft.status[event.properties.sessionID] = { type: "idle" }
          bumpRevision(draft, statusRevisionKey(event.properties.sessionID))
          clearLiveTools(draft, event.properties.sessionID)
        }),
      )
    case "session.error":
      return recordError(set, event.properties.sessionID, event.properties.error, state, directory)
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
function dropSession(set: SetEngineState, info: Session) {
  set(produce((draft) => purgeSession(draft, info.id)))
  clearRecoverableInterruption(info.id)
}

// The session revision bump outlives the purge so an in-flight snapshot taken before the
// deletion cannot resurrect the session.
export function purgeSession(draft: EngineState, id: string) {
  clearPermissionAttentionFor(draft.permissions[id] ?? [])
  delete draft.sessions[id]
  delete draft.transcripts[id]
  delete draft.loaded[id]
  delete draft.permissions[id]
  delete draft.questions[id]
  delete draft.todos[id]
  delete draft.status[id]
  delete draft.activity[id]
  delete draft.errors[id]
  delete draft.sessionModels[id]
  delete draft.cursors[id]
  clearLiveTools(draft, id)
  pruneSessionRevisions(draft, id)
  bumpRevision(draft, sessionRevisionKey(id))
}

// Applies a session-list snapshot. Sessions whose revision advanced while the request was in
// flight keep their live state. When `scope` is present the snapshot is authoritative and
// complete for that directory, so sessions absent from it are purged; partial or failed
// snapshots must never pass a scope.
export function applySessionSnapshot(
  set: SetEngineState,
  input: { sessions: Session[]; captured: Record<string, number>; scope?: { directory: string } | { all: true } },
) {
  const ids = new Set(input.sessions.map((info) => info.id))
  const all = input.scope && "all" in input.scope
  const dir = input.scope && "directory" in input.scope ? normalizeDir(input.scope.directory) : undefined
  const removed: string[] = []
  set(
    produce((draft) => {
      const advanced = (id: string) => revisionAdvanced(draft.revisions, input.captured, sessionRevisionKey(id))
      for (const info of input.sessions) {
        if (advanced(info.id)) continue
        draft.sessions[info.id] = { revert: undefined, share: undefined, ...info }
        const model = (info as Session & { model?: { id: string; providerID: string } }).model
        if (model) draft.sessionModels[info.id] = { providerID: model.providerID, modelID: model.id }
      }
      if (!input.scope) return
      for (const session of Object.values(draft.sessions)) {
        if (ids.has(session.id) || advanced(session.id)) continue
        if (!all && normalizeDir(session.directory) !== dir) continue
        // Scoped listings exclude engine-archived sessions, so their absence is not a deletion.
        // Purging them here would delete-and-reload archived transcripts on every hydration.
        if (!all && (session.time as { archived?: number }).archived) continue
        removed.push(session.id)
        purgeSession(draft, session.id)
      }
    }),
  )
  for (const id of removed) clearRecoverableInterruption(id)
}

// Applies a status snapshot for the given sessions, skipping any whose status a live event
// already moved past the capture point.
export function applyStatusSnapshot(
  set: SetEngineState,
  input: { sessions: Session[]; statuses: Record<string, SessionStatus>; captured: Record<string, number> },
) {
  set(
    produce((draft) => {
      for (const session of input.sessions) {
        if (!draft.sessions[session.id]) continue
        if (revisionAdvanced(draft.revisions, input.captured, statusRevisionKey(session.id))) continue
        const status = input.statuses[session.id] ?? { type: "idle" as const }
        draft.status[session.id] = status
        if (status.type === "idle") clearLiveTools(draft, session.id)
      }
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
      bumpRevision(draft, sessionRevisionKey(moved.sessionID))
    }),
  )
}

function recordError(
  set: SetEngineState,
  sessionID?: string,
  error?: { name: string; data?: unknown },
  state?: EngineState,
  eventDirectory?: string,
) {
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
      bumpRevision(draft, statusRevisionKey(sessionID))
      if (draft.activity[sessionID]) draft.activity[sessionID].current = undefined
      clearLiveTools(draft, sessionID)
      if (error?.name === "MessageAbortedError") return
      draft.errors[sessionID] = message
    }),
  )
  const recoverable = classifyRecoverableError(error)
  if (!recoverable || !state) return
  const session = state.sessions[sessionID]
  const model = sessionModel(state, sessionID, error)
  const directory = session?.directory ?? eventDirectory ?? state.directory
  const workspace = workspaces().find((item) => normalizeDirectory(item.path) === normalizeDirectory(directory))
  recordRecoverableInterruption({
    sessionId: sessionID,
    identity: interruptionIdentity(error as EngineError, model, state.sessionModels[sessionID]?.messageId),
    workspaceId: workspace?.id,
    directory,
    threadTitle: session?.title ?? "",
    parentSessionId: session?.parentID ?? state.links[sessionID],
    providerId: model?.providerID ?? providerFromError(error) ?? "unknown",
    modelId: model?.modelID ?? "unknown",
    errorName: error?.name ?? "Error",
    ...recoverable,
  })
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
      if (info.role === "assistant")
        draft.sessionModels[info.sessionID] = {
          providerID: info.providerID,
          modelID: info.modelID,
          messageId: info.id,
        }
      const list = draft.loaded[info.sessionID] ? draft.transcripts[info.sessionID] : undefined
      if (!list) return
      bumpRevision(draft, messageRevisionKey(info.sessionID, info.id))
      const index = list.findIndex((entry) => entry.info.id === info.id)
      if (index >= 0) list[index].info = info
      else list.push({ info, parts: [] })
    }),
  )
  if (
    info.role === "assistant" &&
    info.time.completed &&
    !(info as { error?: unknown }).error
  )
    clearRecoverableInterruption(info.sessionID, true)
}

function dropMessage(set: SetEngineState, sessionID: string, messageID: string) {
  set(
    produce((draft) => {
      const list = draft.transcripts[sessionID]
      if (!list) return
      bumpRevision(draft, messageRevisionKey(sessionID, messageID))
      draft.transcripts[sessionID] = list.filter((entry) => entry.info.id !== messageID)
    }),
  )
}

function upsertPart(set: SetEngineState, part: Part) {
  const link = spawnLink(part)
  if (link) recordLink(link)
  set(
    produce((draft) => {
      if (link) draft.links[link.child] = link.parent
      if (link && part.type === "tool") {
        const metadata = (("metadata" in part.state ? part.state.metadata : undefined) ?? part.metadata) as
          | { model?: ModelRef }
          | undefined
        if (metadata?.model) draft.sessionModels[link.child] = metadata.model
      }
      if (part.type === "tool") {
        trackActivity(draft, part)
        if (part.state.status === "pending" || part.state.status === "running") draft.liveTools[part.id] = part.sessionID
        else delete draft.liveTools[part.id]
      }
      const entry = draft.transcripts[part.sessionID]?.find((item) => item.info.id === part.messageID)
      if (!entry) return
      bumpRevision(draft, messageRevisionKey(part.sessionID, part.messageID))
      const index = entry.parts.findIndex((existing) => existing.id === part.id)
      if (index >= 0) entry.parts[index] = part
      else entry.parts.push(part)
    }),
  )
}

function sessionModel(state: EngineState, sessionID: string, error?: { data?: unknown }): ModelRef | undefined {
  const known = state.sessionModels[sessionID]
  if (known) return known
  const latest = [...(state.transcripts[sessionID] ?? [])]
    .reverse()
    .find((entry) => entry.info.role === "assistant")?.info
  if (latest?.role === "assistant") return { providerID: latest.providerID, modelID: latest.modelID }
  const data = error?.data && typeof error.data === "object" ? (error.data as Record<string, unknown>) : undefined
  if (typeof data?.providerID === "string" && typeof data.modelID === "string")
    return { providerID: data.providerID, modelID: data.modelID }
}

function providerFromError(error?: { data?: unknown }) {
  const data = error?.data && typeof error.data === "object" ? (error.data as Record<string, unknown>) : undefined
  return typeof data?.providerID === "string" ? data.providerID : undefined
}

function normalizeDirectory(directory: string) {
  return directory.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase()
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
      bumpRevision(draft, messageRevisionKey(ref.sessionID, ref.messageID))
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
      if (entry) {
        bumpRevision(draft, messageRevisionKey(ref.sessionID, ref.messageID))
        entry.parts = entry.parts.filter((part) => part.id !== ref.partID)
      }
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
