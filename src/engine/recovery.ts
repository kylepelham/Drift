import type { Event } from "@opencode-ai/sdk/client"
import type { SetStoreFunction } from "solid-js/store"
import { produce } from "solid-js/store"
import { dropSessionState, normalizeDir, type EngineState, type MessageEntry } from "./store"

export type BufferedEvent = { revision: number; event: Event; directory?: string }
export type RecoveryToken = { generation: number; revision: number }

export type RecoveryCoordinator = ReturnType<typeof createRecoveryCoordinator>

export function createRecoveryCoordinator(
  reducer: (event: Event, directory?: string) => void,
  maxBufferedEvents = 4096,
) {
  let generation = 0
  let revision = 0
  let floor = 0
  let events: BufferedEvent[] = []

  return {
    advance() {
      generation += 1
      floor = revision
      events = []
      return generation
    },
    begin(): RecoveryToken {
      return { generation, revision }
    },
    revision() {
      return revision
    },
    current(token: RecoveryToken) {
      return token.generation === generation && token.revision >= floor
    },
    record(event: Event, directory?: string) {
      revision += 1
      reducer(event, directory)
      events.push({ revision, event, directory })
      if (events.length <= maxBufferedEvents) return
      const removed = events.splice(0, events.length - maxBufferedEvents)
      floor = removed.at(-1)?.revision ?? floor
    },
    commit(token: RecoveryToken, apply: (events: BufferedEvent[]) => void) {
      if (token.generation !== generation || token.revision < floor) return false
      apply(events.filter((entry) => entry.revision > token.revision))
      return true
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
) {
  const touched = new Set(events.filter((entry) => isSessionEvent(entry.event)).map((entry) => sessionID(entry.event)))
  const reported = new Set(sessions.map((session) => session.id))
  set(
    produce((draft) => {
      for (const session of Object.values(draft.sessions)) {
        if (!inScope(session.directory) || reported.has(session.id) || touched.has(session.id)) continue
        dropSessionState(draft, session.id)
      }
      for (const session of sessions) {
        if (touched.has(session.id)) continue
        draft.sessions[session.id] = { revert: undefined, share: undefined, ...session }
      }
    }),
  )
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
) {
  const changed = events.some((entry) => isTranscriptEvent(entry.event) && sessionID(entry.event) === id)
  if (changed && state.loaded[id]) return false
  set(
    produce((draft) => {
      draft.transcripts[id] = entries
      draft.loaded[id] = true
      draft.cursors[id] = cursor
    }),
  )
  return true
}

export function eventInDirectory(entry: BufferedEvent, directory: string) {
  return typeof entry.directory === "string" && normalizeDir(entry.directory) === normalizeDir(directory)
}
