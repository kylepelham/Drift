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
} from "@opencode-ai/sdk/client"
import { createStore, produce, type SetStoreFunction } from "solid-js/store"
import type { Connection } from "./connection"

export type ModelInfo = Model & { family?: string; release_date?: string; variants?: Record<string, unknown> }
export type ProviderInfo = { id: string; name: string; models: Record<string, ModelInfo> }
export type ModelRef = { providerID: string; modelID: string }
export type MessageEntry = { info: Message; parts: Part[] }

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

export type EngineState = {
  connection: Connection
  directory: string
  bootstrappedDirectory: string
  sessions: Record<string, Session>
  status: Record<string, SessionStatus>
  transcripts: Record<string, MessageEntry[]>
  loaded: Record<string, boolean>
  loading: Record<string, boolean>
  permissions: Record<string, Permission[]>
  questions: Record<string, QuestionRequest[]>
  todos: Record<string, Todo[]>
  providers: ProviderInfo[]
  connected: string[]
  defaultModels: Record<string, string>
  agents: Agent[]
  commands: Command[]
  errors: Record<string, string>
  notices: Notice[]
  links: Record<string, string>
  activity: Record<string, SessionActivity>
  cursors: Record<string, string | null>
  version: string
  startupError: string
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
    sessions: {},
    status: {},
    transcripts: {},
    loaded: {},
    loading: {},
    permissions: {},
    questions: {},
    todos: {},
    providers: [],
    connected: [],
    defaultModels: {},
    agents: [],
    commands: [],
    errors: {},
    notices: [],
    links: { ...loadLinks() },
    activity: {},
    startupError: "",
    cursors: {},
    version: "",
  })
}

// Store sets merge; optional keys the engine dropped (revert, share) must clear explicitly.
export function putSession(set: SetStoreFunction<EngineState>, info: Session) {
  set("sessions", info.id, { revert: undefined, share: undefined, ...info })
}

export function putSessions(set: SetStoreFunction<EngineState>, infos: Session[]) {
  set(
    "sessions",
    produce((sessions) => {
      for (const info of infos) sessions[info.id] = { revert: undefined, share: undefined, ...info }
    }),
  )
}

export function dropSessionState(state: EngineState, id: string) {
  delete state.sessions[id]
  delete state.transcripts[id]
  delete state.loaded[id]
  delete state.loading[id]
  delete state.permissions[id]
  delete state.questions[id]
  delete state.todos[id]
  delete state.status[id]
  delete state.activity[id]
  delete state.errors[id]
  delete state.cursors[id]
}

export function modelInfo(state: EngineState, ref: ModelRef | null): ModelInfo | undefined {
  if (!ref) return undefined
  return state.providers.find((p) => p.id === ref.providerID)?.models[ref.modelID]
}

type TokenUsage = { input: number; output: number; reasoning: number; cache: { read: number; write: number }; total?: number }

function tokenCount(tokens: TokenUsage) {
  return tokens.total || tokens.input + tokens.output + tokens.cache.read + tokens.cache.write
}

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
  const maxOutput = Math.min(limits.output || 0, 32000) || 32000
  const reserved = Math.min(20000, maxOutput)
  const usable = limits.input ? Math.max(0, limits.input - reserved) : Math.max(0, context - maxOutput)
  return {
    count,
    context,
    percent: Math.min(100, Math.round((count / context) * 100)),
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
