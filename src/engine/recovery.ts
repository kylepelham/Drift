import type { Event } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { dropSessionState, normalizeDir, type EngineState, type MessageEntry } from "./store"

export type BufferedEvent = { revision: number; event: Event; directory?: string }
export type RecoveryToken = {
  generation: number
  revision: number
  signal: AbortSignal
  readonly events: BufferedEvent[]
  readonly filter: (entry: BufferedEvent) => boolean
  controller: AbortController
  timer?: ReturnType<typeof setTimeout>
  active: boolean
  invalid: boolean
}

export type RecoveryCoordinator = ReturnType<typeof createRecoveryCoordinator>

export function createRecoveryCoordinator(
  reducer: (event: Event, directory?: string) => void,
  maxBufferedEvents = 512,
) {
  let generation = 0
  let revision = 0
  const active = new Set<RecoveryToken>()

  function finish(token: RecoveryToken) {
    token.active = false
    active.delete(token)
    if (token.timer) clearTimeout(token.timer)
  }

  function current(token: RecoveryToken) {
    return token.generation === generation && !token.invalid && !token.signal.aborted
  }

  return {
    advance() {
      generation += 1
      for (const token of active) {
        token.invalid = true
        token.controller.abort("recovery generation changed")
        finish(token)
      }
      return generation
    },
    begin(filter: (entry: BufferedEvent) => boolean = () => true, timeoutMs = 10_000): RecoveryToken {
      const controller = new AbortController()
      const token: RecoveryToken = {
        generation,
        revision,
        signal: controller.signal,
        events: [],
        filter,
        controller,
        active: true,
        invalid: false,
      }
      token.timer = setTimeout(() => {
        token.invalid = true
        controller.abort("recovery request timed out")
        finish(token)
      }, timeoutMs)
      active.add(token)
      return token
    },
    revision() {
      return revision
    },
    generation() {
      return generation
    },
    current(token: RecoveryToken) {
      return current(token)
    },
    record(event: Event, directory?: string) {
      revision += 1
      reducer(event, directory)
      const entry = { revision, event, directory }
      for (const token of active) {
        if (!token.filter(entry)) continue
        token.events.push(entry)
        if (token.events.length <= maxBufferedEvents) continue
        token.invalid = true
        token.controller.abort("recovery event buffer exceeded")
        finish(token)
      }
    },
    commit(token: RecoveryToken, apply: (events: BufferedEvent[]) => void) {
      finish(token)
      if (!current(token)) return false
      apply(token.events)
      return true
    },
    cancel(token: RecoveryToken) {
      finish(token)
    },
    replay(entries: BufferedEvent[]) {
      for (const entry of entries) reducer(entry.event, entry.directory)
    },
  }
}

export function sessionID(event: Event): string | undefined {
  const raw = event as unknown as { type: string; properties?: Record<string, unknown> }
  const properties = raw.properties ?? {}
  if (typeof properties.sessionID === "string") return properties.sessionID
  const info = properties.info as { id?: string; sessionID?: string } | undefined
  if (raw.type.startsWith("session.")) return info?.id
  if (typeof info?.sessionID === "string") return info.sessionID
  const part = properties.part as { sessionID?: string } | undefined
  return part?.sessionID
}

export function isSessionEvent(event: Event) {
  const type = (event as { type: string }).type
  return (
    type === "session.created" ||
    type === "session.updated" ||
    type === "session.deleted" ||
    type === "session.next.moved"
  )
}

export function isStatusEvent(event: Event) {
  const type = (event as { type: string }).type
  return type === "session.status" || type === "session.idle" || type === "session.error" || type === "session.deleted"
}

export function isTranscriptEvent(event: Event) {
  const type = (event as { type: string }).type
  return type.startsWith("message.") || type === "session.deleted"
}

export function applySessionSnapshot(
  set: SetStoreFunction<EngineState>,
  sessions: EngineState["sessions"][string][],
  inScope: (directory: string) => boolean,
  events: BufferedEvent[],
  replay: (events: BufferedEvent[]) => void = () => undefined,
) {
  const moved = new Set(
    events
      .filter((entry) => (entry.event as unknown as { type: string }).type === "session.next.moved")
      .map((entry) => sessionID(entry.event)),
  )
  const touched = new Set(
    events
      .filter(
        (entry) =>
          isSessionEvent(entry.event) && (entry.event as unknown as { type: string }).type !== "session.next.moved",
      )
      .map((entry) => sessionID(entry.event)),
  )
  const reported = new Set(sessions.map((session) => session.id))
  set(
    produce((draft) => {
      for (const session of Object.values(draft.sessions)) {
        if (!inScope(session.directory) || reported.has(session.id) || touched.has(session.id) || moved.has(session.id))
          continue
        dropSessionState(draft, session.id)
      }
      for (const session of sessions) {
        if (touched.has(session.id)) continue
        draft.sessions[session.id] = { revert: undefined, share: undefined, ...session }
      }
    }),
  )
  replay(events.filter((entry) => (entry.event as unknown as { type: string }).type === "session.next.moved"))
}

export function applyStatusSnapshot(
  set: SetStoreFunction<EngineState>,
  sessions: EngineState["sessions"][string][],
  statuses: EngineState["status"],
  events: BufferedEvent[],
) {
  const touched = new Set(events.filter((entry) => isStatusEvent(entry.event)).map((entry) => sessionID(entry.event)))
  set(
    produce((draft) => {
      for (const session of sessions) {
        if (!touched.has(session.id)) draft.status[session.id] = statuses[session.id] ?? { type: "idle" }
      }
    }),
  )
}

export function applyTranscriptSnapshot(
  state: EngineState,
  set: SetStoreFunction<EngineState>,
  id: string,
  entries: MessageEntry[],
  cursor: string | null,
  events: BufferedEvent[],
  loadedAtStart = state.loaded[id] === true,
) {
  const relevant = events.filter((entry) => isTranscriptEvent(entry.event) && sessionID(entry.event) === id)
  if (relevant.some((entry) => (entry.event as unknown as { type: string }).type === "session.deleted"))
    return "deleted" as const
  const existing = loadedAtStart ? (state.transcripts[id] ?? []) : []
  const deltas = relevant.filter(
    (entry) => (entry.event as unknown as { type: string }).type === "message.part.delta",
  )
  if (deltas.length && (!loadedAtStart || deltas.some((entry) => !deltaWasApplied(existing, entry.event))))
    return "retry" as const
  const snapshotIDs = new Set(entries.map((entry) => entry.info.id))
  const touchedIDs = new Set(relevant.map((entry) => messageID(entry.event)).filter((value) => value !== undefined))
  const preservedIDs = new Set(
    existing
      .filter((entry) => !snapshotIDs.has(entry.info.id) || touchedIDs.has(entry.info.id))
      .map((entry) => entry.info.id),
  )
  set(
    produce((draft) => {
      draft.transcripts[id] = [
        ...existing.filter((entry) => preservedIDs.has(entry.info.id)),
        ...entries.filter((entry) => !preservedIDs.has(entry.info.id)),
      ]
      for (const entry of relevant) applyIdempotentTranscriptEvent(draft.transcripts[id], entry.event)
      draft.transcripts[id].sort((a, b) => a.info.id.localeCompare(b.info.id))
      draft.loaded[id] = true
      draft.cursors[id] = cursor
    }),
  )
  return "applied" as const
}

function deltaWasApplied(entries: MessageEntry[], event: Event) {
  const properties = (event as unknown as { properties: Record<string, unknown> }).properties
  const messageID = properties.messageID
  const partID = properties.partID
  const field = properties.field
  if (typeof messageID !== "string" || typeof partID !== "string" || typeof field !== "string") return false
  const part = entries
    .find((entry) => entry.info.id === messageID)
    ?.parts.find((candidate) => candidate.id === partID) as unknown as Record<string, unknown> | undefined
  return typeof part?.[field] === "string"
}

function applyIdempotentTranscriptEvent(entries: MessageEntry[], event: Event) {
  const raw = event as unknown as { type: string; properties: Record<string, unknown> }
  if (raw.type === "message.updated") {
    const info = raw.properties.info as MessageEntry["info"]
    const index = entries.findIndex((entry) => entry.info.id === info.id)
    if (index >= 0) entries[index].info = info
    else entries.push({ info, parts: [] })
    return
  }
  if (raw.type === "message.removed") {
    const index = entries.findIndex((entry) => entry.info.id === raw.properties.messageID)
    if (index >= 0) entries.splice(index, 1)
    return
  }
  if (raw.type === "message.part.updated") {
    const part = raw.properties.part as MessageEntry["parts"][number]
    const message = entries.find((entry) => entry.info.id === part.messageID)
    if (!message) return
    const index = message.parts.findIndex((candidate) => candidate.id === part.id)
    if (index >= 0) message.parts[index] = part
    else message.parts.push(part)
    return
  }
  if (raw.type === "message.part.removed") {
    const message = entries.find((entry) => entry.info.id === raw.properties.messageID)
    if (message) message.parts = message.parts.filter((part) => part.id !== raw.properties.partID)
  }
}

function messageID(event: Event): string | undefined {
  const properties = (event as unknown as { properties?: Record<string, unknown> }).properties ?? {}
  if (typeof properties.messageID === "string") return properties.messageID
  const info = properties.info as { id?: string } | undefined
  if (typeof info?.id === "string") return info.id
  const part = properties.part as { messageID?: string } | undefined
  return part?.messageID
}

export function eventInDirectory(entry: BufferedEvent, directory: string) {
  const target = normalizeDir(directory)
  if (typeof entry.directory === "string" && normalizeDir(entry.directory) === target) return true
  const properties = (entry.event as unknown as { properties?: Record<string, unknown> }).properties ?? {}
  const info = properties.info as { directory?: string } | undefined
  if (typeof info?.directory === "string" && normalizeDir(info.directory) === target) return true
  const location = properties.location as { directory?: string } | undefined
  return typeof location?.directory === "string" && normalizeDir(location.directory) === target
}
