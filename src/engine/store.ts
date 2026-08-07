import type {
  Agent,
  Command,
  Message,
  Model,
  Part,
  Permission,
  Session,
  SessionStatus,
  Todo,
  ToolPart,
} from "@opencode-ai/sdk/client"
import { createStore, produce, type SetStoreFunction } from "solid-js/store"
import type { Connection } from "./connection"

export type ModelInfo = Model & { family?: string; release_date?: string; variants?: Record<string, unknown> }
export type ProviderInfo = { id: string; name: string; models: Record<string, ModelInfo> }
export type ModelRef = { providerID: string; modelID: string }
export type MessageEntry = { info: Message; parts: Part[] }

export function interruptStaleTools(entries: MessageEntry[], liveTools: Readonly<Record<string, string>>, error = "Interrupted") {
  return entries.map((entry) => {
    let changed = false
    const parts = entry.parts.map((part) => {
      if (part.type !== "tool" || (part.state.status !== "pending" && part.state.status !== "running")) return part
      if (liveTools[part.id] === part.sessionID) return part
      changed = true
      const completed = (entry.info as { time: { completed?: number } }).time.completed
      const start = "time" in part.state ? part.state.time.start : undefined
      const metadata = "metadata" in part.state ? part.state.metadata : undefined
      return {
        ...part,
        state: {
          status: "error",
          input: part.state.input,
          error,
          metadata,
          ...(start === undefined ? {} : { time: { start, end: Math.max(start, completed ?? start) } }),
        },
      } as ToolPart
    })
    return changed ? { ...entry, parts } : entry
  })
}

export function messageText(entry: MessageEntry) {
  return entry.parts
    .flatMap((part) => (part.type === "text" && !part.synthetic ? [part.text] : []))
    .join("\n")
}

export function previousUserMessage(entries: MessageEntry[], before?: string) {
  return entries
    .filter((entry) => entry.info.role === "user" && (!before || entry.info.id < before))
    .sort((a, b) => b.info.id.localeCompare(a.info.id))[0]
}

export function nextUserMessage(entries: MessageEntry[], after: string) {
  return entries
    .filter((entry) => entry.info.role === "user" && entry.info.id > after)
    .sort((a, b) => a.info.id.localeCompare(b.info.id))[0]
}

export type SessionActivity = { tools: number; lastPartId: string; current?: string }

export type QuestionInfo = {
  question: string
  header: string
  options: { label: string; description: string }[]
  multiple?: boolean
  custom?: boolean
}
export type QuestionRequest = {
  id: string
  sessionID: string
  questions: QuestionInfo[]
  tool?: { messageID: string; callID: string }
  directory?: string
}

export type Notice = {
  id: string
  title?: string
  message: string
  variant: "info" | "success" | "warning" | "error"
  created: number
  duration: number
}

export type AskKind = "permission" | "question"

export type EngineState = {
  connection: Connection
  directory: string
  bootstrappedDirectory: string
  sessionSnapshotDirectory: string
  sessionSnapshotAll: boolean
  sessionSnapshotEpoch: number
  providerSnapshotEpoch: number
  sessions: Record<string, Session>
  status: Record<string, SessionStatus>
  transcripts: Record<string, MessageEntry[]>
  loaded: Record<string, boolean>
  permissions: Record<string, Permission[]>
  questions: Record<string, QuestionRequest[]>
  askRevisions: Record<string, number>
  todos: Record<string, Todo[]>
  providers: ProviderInfo[]
  connected: string[]
  defaultModels: Record<string, string>
  agents: Agent[]
  commands: Command[]
  errors: Record<string, string>
  sessionModels: Record<string, ModelRef & { messageId?: string }>
  notices: Notice[]
  links: Record<string, string>
  activity: Record<string, SessionActivity>
  liveTools: Record<string, string>
  cursors: Record<string, string | null>
  revisions: Record<string, number>
  version: string
  startupError: string
  engineError: string
  engineRestarting: boolean
}

let storedLinks: Record<string, string> | undefined

function loadLinks(): Record<string, string> {
  if (storedLinks) return storedLinks
  try {
    storedLinks = JSON.parse(localStorage.getItem("drift.links") ?? "{}") as Record<string, string>
  } catch {
    storedLinks = {}
  }
  return storedLinks
}

export function recordLink(link: { child: string; parent: string }) {
  const links = loadLinks()
  if (links[link.child] === link.parent) return
  links[link.child] = link.parent
  localStorage.setItem("drift.links", JSON.stringify(links))
}

export function createEngineState() {
  return createStore<EngineState>({
    connection: "idle",
    directory: "",
    bootstrappedDirectory: "",
    sessionSnapshotDirectory: "",
    sessionSnapshotAll: false,
    sessionSnapshotEpoch: 0,
    providerSnapshotEpoch: 0,
    sessions: {},
    status: {},
    transcripts: {},
    loaded: {},
    permissions: {},
    questions: {},
    askRevisions: {},
    todos: {},
    providers: [],
    connected: [],
    defaultModels: {},
    agents: [],
    commands: [],
    errors: {},
    sessionModels: {},
    notices: [],
    links: { ...loadLinks() },
    activity: {},
    liveTools: {},
    startupError: "",
    engineError: "",
    engineRestarting: false,
    cursors: {},
    revisions: {},
    version: "",
  })
}

// Store sets merge; optional keys the engine dropped (revert, share) must clear explicitly.
export function putSession(set: SetStoreFunction<EngineState>, info: Session) {
  set(
    produce((draft) => {
      draft.sessions[info.id] = { revert: undefined, share: undefined, ...info }
      const model = (info as Session & { model?: { id: string; providerID: string } }).model
      if (model) draft.sessionModels[info.id] = { providerID: model.providerID, modelID: model.id }
      bumpRevision(draft, sessionRevisionKey(info.id))
    }),
  )
}

export function putSessions(set: SetStoreFunction<EngineState>, infos: Session[]) {
  set(
    "sessions",
    produce((sessions) => {
      for (const info of infos) sessions[info.id] = { revert: undefined, share: undefined, ...info }
    }),
  )
  set(
    "sessionModels",
    produce((models) => {
      for (const info of infos) {
        const model = (info as Session & { model?: { id: string; providerID: string } }).model
        if (model) models[info.id] = { providerID: model.providerID, modelID: model.id }
      }
    }),
  )
}

// The engine caps session listings at this size; a shorter page means the snapshot covered its
// entire scope, so sessions absent from it can be reconciled away.
export const sessionSnapshotLimit = 100

// Monotonic counters bumped by every live reduction that touches the keyed slice. Snapshot writes
// compare them against a capture taken before the HTTP request started, so state that raced ahead
// of the snapshot is never overwritten by it.
export function sessionRevisionKey(sessionID: string) {
  return `session\0${sessionID}`
}

export function statusRevisionKey(sessionID: string) {
  return `status\0${sessionID}`
}

export function messageRevisionKey(sessionID: string, messageID: string) {
  return `message\0${sessionID}\0${messageID}`
}

export function bumpRevision(draft: EngineState, key: string) {
  draft.revisions[key] = (draft.revisions[key] ?? 0) + 1
}

export function captureRevisions(state: EngineState): Record<string, number> {
  return { ...state.revisions }
}

export function revisionAdvanced(current: Record<string, number>, captured: Record<string, number>, key: string) {
  return (current[key] ?? 0) !== (captured[key] ?? 0)
}

// The session revision survives so an in-flight snapshot cannot resurrect a purged session.
export function pruneSessionRevisions(draft: EngineState, sessionID: string) {
  delete draft.revisions[statusRevisionKey(sessionID)]
  const prefix = `message\0${sessionID}\0`
  for (const key of Object.keys(draft.revisions)) if (key.startsWith(prefix)) delete draft.revisions[key]
}

// Merges a transcript snapshot with what the event stream did while the request was in flight:
// the snapshot is authoritative for untouched messages, live state wins for touched ones.
export function mergeTranscriptSnapshot(
  live: MessageEntry[] | undefined,
  snapshot: MessageEntry[],
  sessionID: string,
  captured: Record<string, number>,
  revisions: Record<string, number>,
) {
  const advanced = (messageID: string) => revisionAdvanced(revisions, captured, messageRevisionKey(sessionID, messageID))
  const liveById = new Map((live ?? []).map((entry) => [entry.info.id, entry]))
  const snapshotIds = new Set(snapshot.map((entry) => entry.info.id))
  const merged = snapshot.flatMap((entry) => {
    const current = liveById.get(entry.info.id)
    if (!advanced(entry.info.id)) {
      // Reuse the live object when the content is unchanged: transcript rows are referentially
      // keyed, so handing the UI a fresh-but-identical object would remount every visible row
      // (a full-transcript flash on each reconnect hydration).
      return [current && JSON.stringify(current) === JSON.stringify(entry) ? current : entry]
    }
    return current ? [current] : []
  })
  for (const entry of live ?? []) if (advanced(entry.info.id) && !snapshotIds.has(entry.info.id)) merged.push(entry)
  return merged.sort((a, b) => a.info.id.localeCompare(b.info.id))
}

export function modelInfo(state: EngineState, ref: ModelRef | null): ModelInfo | undefined {
  if (!ref) return undefined
  return state.providers.find((p) => p.id === ref.providerID)?.models[ref.modelID]
}

type TokenUsage = { input: number; output: number; reasoning: number; cache: { read: number; write: number }; total?: number }

function tokenCount(tokens: TokenUsage) {
  return tokens.total || tokens.input + tokens.output + tokens.cache.read + tokens.cache.write
}

// Ceiling on how much of the context window is set aside for the model's own reply, and the slice
// of that reserved for compaction headroom. Both mirror the engine's session/overflow.ts - changing
// one here without changing it there desynchronizes the meter from actual compaction.
const maxOutputTokens = 32000
const compactionReserveTokens = 20000
const percentScale = 100

// Mirrors the engine's session/overflow.ts so the meter predicts the same compaction point.
// Limits come from the model the next prompt would use; token counts from the last reply.
export function contextStats(state: EngineState, sessionId: string, modelRef?: ModelRef | null) {
  const entries = state.transcripts[sessionId] ?? []
  const last = [...entries].reverse().find((entry) => {
    if (entry.info.role !== "assistant" || !("tokens" in entry.info)) return false
    return tokenCount(entry.info.tokens as TokenUsage) > 0
  })
  if (!last || !("tokens" in last.info)) return null
  const tokens = last.info.tokens as TokenUsage
  const count = tokenCount(tokens)
  const model =
    modelInfo(state, modelRef ?? null) ?? modelInfo(state, { providerID: last.info.providerID, modelID: last.info.modelID })
  const limits = (model?.limit ?? {}) as { context?: number; output?: number; input?: number }
  const context = limits.context ?? 0
  if (!context || !count) return null
  const maxOutput = Math.min(limits.output || 0, maxOutputTokens) || maxOutputTokens
  const reserved = Math.min(compactionReserveTokens, maxOutput)
  const usable = limits.input ? Math.max(0, limits.input - reserved) : Math.max(0, context - maxOutput)
  return {
    count,
    context,
    percent: Math.min(percentScale, Math.round((count / context) * percentScale)),
    untilCompaction: Math.max(0, usable - count),
    cost: (state.sessions[sessionId] as { cost?: number } | undefined)?.cost ?? 0,
  }
}

export function spawnLink(part: Part): { child: string; parent: string } | undefined {
  if (part.type !== "tool" || (part.tool !== "task" && part.tool !== "spawn_thread")) return
  const state = part.state
  const meta = (("metadata" in state ? state.metadata : undefined) ?? part.metadata) as { sessionId?: string }
  if (!meta?.sessionId) return
  return { child: meta.sessionId, parent: part.sessionID }
}

export function sessionBusy(state: EngineState, id: string) {
  const status = state.status[id]?.type
  return status === "busy" || status === "retry"
}

export function normalizeDir(path: string) {
  return path.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase()
}

export function askRevisionKey(kind: AskKind, directory: string) {
  return `${kind}\0${normalizeDir(directory)}`
}

export function askRevision(state: EngineState, kind: AskKind, directory: string) {
  return state.askRevisions[askRevisionKey(kind, directory)] ?? 0
}

export function bumpAskRevision(state: EngineState, kind: AskKind, directory?: string) {
  if (!directory) return
  const key = askRevisionKey(kind, directory)
  state.askRevisions[key] = (state.askRevisions[key] ?? 0) + 1
}

export function sessionsFor(state: EngineState, directory: string) {
  const dir = normalizeDir(directory)
  return Object.values(state.sessions)
    .filter((session) => !session.parentID && normalizeDir(session.directory) === dir)
    .sort((a, b) => b.time.updated - a.time.updated)
}

export function childrenOf(state: EngineState, parentId: string) {
  return Object.values(state.sessions)
    .filter((session) => session.parentID === parentId)
    .sort((a, b) => a.time.created - b.time.created)
}

const providerPriority = ["anthropic", "openai", "opencode", "github-copilot", "google", "zai", "xai"]

export function resolveModel(state: EngineState, pref: ModelRef | null): ModelRef | null {
  if (pref && state.providers.some((p) => p.id === pref.providerID && pref.modelID in p.models)) return pref
  const connected = state.providers.filter((p) => state.connected.includes(p.id))
  const pool = connected.length ? connected : state.providers
  const rank = (id: string) => {
    const index = providerPriority.indexOf(id)
    return index < 0 ? providerPriority.length : index
  }
  for (const provider of [...pool].sort((a, b) => rank(a.id) - rank(b.id))) {
    const usable = Object.values(provider.models).filter((model) => model.capabilities.toolcall)
    if (!usable.length) continue
    const preferred = provider.models[state.defaultModels[provider.id] ?? ""]
    const model = preferred?.capabilities.toolcall ? preferred : usable[0]
    return { providerID: provider.id, modelID: model.id }
  }
  return null
}
