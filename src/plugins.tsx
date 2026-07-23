import type { Part, ToolPart } from "@opencode-ai/sdk/client"
import { createEffect, createSignal, onCleanup, onMount, untrack } from "solid-js"
import type { Engine } from "./engine"
import type { QuestionInfo } from "./engine/store"
import { pushAsk } from "./state/asks"
import { selectedSession, selectSession } from "./state/selection"
import { shellInvoke } from "./state/store"
import { theme } from "./state/theme"
import { activeWorkspace } from "./state/workspaces"
import type { Workspace } from "./state/store"
import {
  openFile,
  registerToolContextActions,
  type FileLocation,
  type ToolContextActionProvider,
} from "./tool-actions"

type WorkspaceInfo = Pick<Workspace, "id" | "name" | "path">
type Context = {
  connection: Engine["state"]["connection"]
  sessionId: string | null
  workspace: WorkspaceInfo | null
}
type HookEvents = {
  "composer.submit": { text: string; sessionId: string | null; workspace: WorkspaceInfo | null }
  "theme.changed": { theme: string }
  "thread.created": { sessionId: string }
  "thread.selected": { sessionId: string | null }
  "thread.archived": { sessionId: string }
  "workspace.changed": { workspace: WorkspaceInfo | null }
  "message.rendered": { sessionId: string; messageId: string; role: string }
  "permission.requested": { sessionId: string; permissionId: string; title: string; type: string; patterns: string[] }
  "question.requested": { sessionId: string; requestId: string; headers: string[] }
  "session.idle": { sessionId: string }
}
type HookName = keyof HookEvents
type Hook<K extends HookName> = (event: HookEvents[K]) => unknown | Promise<unknown>
type AnyHook = (event: never) => unknown | Promise<unknown>
export type ToolRenderer = (part: ToolPart) => Node | string | null
export type PartRenderer = (part: Part) => Node | string | null

export type DriftPluginApi = {
  version: 1
  context: () => Context
  on: <K extends HookName>(name: K, hook: Hook<K>) => () => void
  registerToolRenderer: (tool: string, renderer: ToolRenderer) => () => void
  registerPartRenderer: (type: string, renderer: PartRenderer) => () => void
  registerToolContextActions: (tool: string, provider: ToolContextActionProvider) => () => void
  ask: (question: QuestionInfo | QuestionInfo[]) => Promise<string[][] | null>
  files: {
    open: (path: string, location?: FileLocation) => Promise<{ positioned: boolean }>
  }
  threads: {
    create: () => Promise<string | undefined>
    select: (sessionId: string | null) => void
  }
}

type PluginModule = { default?: (api: DriftPluginApi) => void | (() => void) | Promise<void | (() => void)> }
type Config = { plugins?: unknown }

const hooks = new Map<HookName, Set<AnyHook>>()
const renderers = new Map<string, ToolRenderer>()
const partRenderers = new Map<string, PartRenderer>()
const cleanups = new Set<() => void>()
const [rendererVersion, setRendererVersion] = createSignal(0)
let generation = 0

export function pluginPaths(source: string) {
  const config = JSON.parse(source) as Config | null
  if (!config || typeof config !== "object") return []
  if (!Array.isArray(config.plugins)) return []
  return config.plugins.filter((path): path is string => typeof path === "string").map(safePluginPath).filter(Boolean)
}

function safePluginPath(path: string) {
  const normalized = path.replaceAll("\\", "/")
  if (!/\.m?js$/i.test(normalized) || normalized.startsWith("/") || normalized.includes(":")) return ""
  if (normalized.split("/").includes("..")) return ""
  return normalized
}

function on<K extends HookName>(name: K, hook: Hook<K>) {
  const group = hooks.get(name) ?? new Set<AnyHook>()
  hooks.set(name, group)
  group.add(hook as AnyHook)
  return () => group.delete(hook as AnyHook)
}

async function emit<K extends HookName>(name: K, event: HookEvents[K]) {
  for (const hook of hooks.get(name) ?? []) {
    try {
      await hook(event as never)
    } catch (error) {
      console.warn(`[Drift] Plugin hook ${name} failed`, error)
    }
  }
}

export async function transformComposerSubmit(event: HookEvents["composer.submit"]) {
  let text = event.text
  for (const hook of hooks.get("composer.submit") ?? []) {
    try {
      const result = await hook({ ...event, text } as never)
      if (result === false) return null
      if (typeof result === "string") text = result
    } catch (error) {
      console.warn("[Drift] Plugin hook composer.submit failed", error)
    }
  }
  return text
}

export function emitThreadCreated(sessionId: string) {
  void emit("thread.created", { sessionId })
}

export function emitThreadArchived(sessionId: string) {
  void emit("thread.archived", { sessionId })
}

export function emitMessageRendered(event: HookEvents["message.rendered"]) {
  void emit("message.rendered", event)
}

export function hasToolRenderer(tool: string) {
  rendererVersion()
  return renderers.has(tool)
}

export function PluginToolView(props: { part: ToolPart }) {
  let root!: HTMLDivElement
  createEffect(() => {
    rendererVersion()
    props.part.state.status
    const output = renderers.get(props.part.tool)?.(props.part)
    root.replaceChildren()
    if (typeof output === "string") root.textContent = output
    else if (output) root.append(output)
  })
  return <div ref={root} class="text-sm" />
}

function registerToolRenderer(tool: string, renderer: ToolRenderer) {
  renderers.set(tool, renderer)
  setRendererVersion((value) => value + 1)
  const off = () => {
    if (renderers.get(tool) !== renderer) return
    renderers.delete(tool)
    setRendererVersion((value) => value + 1)
  }
  return off
}

export function hasPartRenderer(type: string) {
  rendererVersion()
  return partRenderers.has(type)
}

export function PluginPartView(props: { part: Part }) {
  let root!: HTMLDivElement
  createEffect(() => {
    rendererVersion()
    JSON.stringify(props.part)
    const output = partRenderers.get(props.part.type)?.(props.part)
    root.replaceChildren()
    if (typeof output === "string") root.textContent = output
    else if (output) root.append(output)
  })
  return <div ref={root} class="text-sm" />
}

function registerPartRenderer(type: string, renderer: PartRenderer) {
  partRenderers.set(type, renderer)
  setRendererVersion((value) => value + 1)
  const off = () => {
    if (partRenderers.get(type) !== renderer) return
    partRenderers.delete(type)
    setRendererVersion((value) => value + 1)
  }
  return off
}

function unloadPlugins() {
  for (const cleanup of cleanups) {
    try {
      cleanup()
    } catch (error) {
      console.warn("[Drift] Plugin cleanup failed", error)
    }
  }
  cleanups.clear()
  hooks.clear()
  renderers.clear()
  partRenderers.clear()
  setRendererVersion((value) => value + 1)
}

async function importPlugin(source: string) {
  const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }))
  try {
    return (await import(/* @vite-ignore */ url)) as PluginModule
  } finally {
    URL.revokeObjectURL(url)
  }
}

async function readConfigFile(path: string) {
  const invoke = shellInvoke()
  if (invoke) return invoke<string | null>("config_read", { path })
  return localStorage.getItem(`drift.config:${path}`)
}

function createPluginApi(engine: Engine) {
  const owned = new Set<() => void>()
  const track = (cleanup: () => void) => {
    owned.add(cleanup)
    return () => {
      cleanup()
      owned.delete(cleanup)
    }
  }
  const api: DriftPluginApi = {
    version: 1,
    context: () => ({ connection: engine.state.connection, sessionId: selectedSession(), workspace: activeWorkspace() }),
    on: (name, hook) => track(on(name, hook)),
    registerToolRenderer: (tool, renderer) => track(registerToolRenderer(tool, renderer)),
    registerPartRenderer: (type, renderer) => track(registerPartRenderer(type, renderer)),
    registerToolContextActions: (tool, provider) => track(registerToolContextActions(tool, provider)),
    ask: (question) => pushAsk(Array.isArray(question) ? question : [question], selectedSession()),
    files: { open: openFile },
    threads: {
      create: async () => {
        const session = await engine.actions.newSession()
        if (!session) return
        selectSession(session.id)
        emitThreadCreated(session.id)
        return session.id
      },
      select: selectSession,
    },
  }
  return { api, cleanup: () => owned.forEach((dispose) => dispose()) }
}

async function loadPlugins(engine: Engine, current: number) {
  const config = await readConfigFile("drift.json")
  if (current !== generation || !config) return
  for (const path of pluginPaths(config)) {
    const file = await readConfigFile(path)
    if (current !== generation) return
    if (!file) {
      console.warn(`[Drift] Plugin does not exist: ${path}`)
      continue
    }
    const owner = createPluginApi(engine)
    try {
      const plugin = await importPlugin(file)
      if (typeof plugin.default !== "function") throw new Error("no default function")
      const cleanup = await plugin.default(owner.api)
      const dispose = () => {
        cleanup?.()
        owner.cleanup()
      }
      if (current !== generation) {
        dispose()
        return
      }
      cleanups.add(dispose)
    } catch (error) {
      owner.cleanup()
      console.warn(`[Drift] Could not load plugin ${path}`, error)
    }
  }
  await emit("workspace.changed", { workspace: activeWorkspace() })
  await emit("thread.selected", { sessionId: selectedSession() })
  await emit("theme.changed", { theme: theme() })
}

export function PluginHost(props: { engine: Engine }) {
  onMount(() => {
    const current = ++generation
    untrack(unloadPlugins)
    void loadPlugins(props.engine, current).catch((error) =>
      console.warn("[Drift] Could not load plugins", error),
    )
  })
  createEffect(() => {
    const workspace = activeWorkspace()
    untrack(() => void emit("workspace.changed", { workspace }))
  })
  createEffect(() => {
    const sessionId = selectedSession()
    untrack(() => void emit("thread.selected", { sessionId }))
  })
  createEffect(() => {
    const current = theme()
    untrack(() => void emit("theme.changed", { theme: current }))
  })
  const seenPermissions = new Set<string>()
  createEffect(() => {
    const all = Object.values(props.engine.state.permissions).flat()
    const present = new Set(all.map((permission) => permission.id))
    for (const id of seenPermissions) if (!present.has(id)) seenPermissions.delete(id)
    for (const permission of all) {
      if (seenPermissions.has(permission.id)) continue
      seenPermissions.add(permission.id)
      untrack(() =>
        void emit("permission.requested", {
          sessionId: permission.sessionID,
          permissionId: permission.id,
          title: permission.title,
          type: permission.type,
          patterns: [permission.pattern ?? []].flat(),
        }),
      )
    }
  })
  const seenQuestions = new Set<string>()
  createEffect(() => {
    const all = Object.values(props.engine.state.questions).flat()
    const present = new Set(all.map((question) => question.id))
    for (const id of seenQuestions) if (!present.has(id)) seenQuestions.delete(id)
    for (const question of all) {
      if (seenQuestions.has(question.id)) continue
      seenQuestions.add(question.id)
      untrack(() =>
        void emit("question.requested", {
          sessionId: question.sessionID,
          requestId: question.id,
          headers: question.questions.map((item) => item.header),
        }),
      )
    }
  })
  const busySessions = new Set<string>()
  createEffect(() => {
    for (const [sessionId, status] of Object.entries(props.engine.state.status)) {
      const busy = status.type === "busy" || status.type === "retry"
      if (busy) busySessions.add(sessionId)
      else if (busySessions.delete(sessionId)) untrack(() => void emit("session.idle", { sessionId }))
    }
  })
  onCleanup(() => {
    generation++
    unloadPlugins()
  })
  return null
}
