import type { Agent, Message, Model, Part, Permission, Session, SessionStatus, Todo } from "@opencode-ai/sdk/client"
import { createStore } from "solid-js/store"
import type { Connection } from "./connection"

export type ProviderInfo = { id: string; name: string; models: Record<string, Model> }
export type ModelInfo = Model
export type ModelRef = { providerID: string; modelID: string }
export type MessageEntry = { info: Message; parts: Part[] }

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
  errors: Record<string, string>
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
    errors: {},
  })
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
