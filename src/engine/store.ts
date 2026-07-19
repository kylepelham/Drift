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
import { createStore, type SetStoreFunction } from "solid-js/store"
import type { Connection } from "./connection"

export type ModelInfo = Model & { variants?: Record<string, unknown> }
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

export type EngineState = {
  connection: Connection
  directory: string
  sessions: Record<string, Session>
  status: Record<string, SessionStatus>
  transcripts: Record<string, MessageEntry[]>
  loaded: Record<string, boolean>
  permissions: Record<string, Permission[]>
  todos: Record<string, Todo[]>
  providers: ProviderInfo[]
  connected: string[]
  defaultModels: Record<string, string>
  agents: Agent[]
  commands: Command[]
  errors: Record<string, string>
  links: Record<string, string>
  activity: Record<string, SessionActivity>
}

export function createEngineState() {
  return createStore<EngineState>({
    connection: "idle",
    directory: "",
    sessions: {},
    status: {},
    transcripts: {},
    loaded: {},
    permissions: {},
    todos: {},
    providers: [],
    connected: [],
    defaultModels: {},
    agents: [],
    commands: [],
    errors: {},
    links: {},
    activity: {},
  })
}

// Store sets merge; optional keys the engine dropped (revert, share) must clear explicitly.
export function putSession(set: SetStoreFunction<EngineState>, info: Session) {
  set("sessions", info.id, { revert: undefined, share: undefined, ...info })
}

export function modelInfo(state: EngineState, ref: ModelRef | null): ModelInfo | undefined {
  if (!ref) return undefined
  return state.providers.find((p) => p.id === ref.providerID)?.models[ref.modelID]
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
