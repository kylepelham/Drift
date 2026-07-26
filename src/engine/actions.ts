import { createOpencodeClient, type OpencodeClient, type Permission, type Session } from "@opencode-ai/sdk/client"
import { createOpencodeClient as createControlClient } from "@opencode-ai/sdk/v2/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { sleep, type EngineTarget } from "./connection"
import { pushNotice } from "./events"
import {
  applySessionSnapshot,
  applyTranscriptSnapshot,
  createRecoveryCoordinator,
  eventInDirectory,
  isSessionEvent,
  isTranscriptEvent,
  sessionID,
  type BufferedEvent,
  type RecoveryCoordinator,
} from "./recovery"
import type { MessageEntry } from "./store"
import {
  dropSessionState,
  normalizeDir,
  putSession,
  recordLink,
  sessionBusy,
  spawnLink,
  type EngineState,
  type ModelRef,
  type Notice,
  type ProviderInfo,
  type QuestionRequest,
} from "./store"

export type PromptFile = {
  filename?: string
  mime: string
  url: string
  source?: { type: "file"; path: string; text: { value: string; start: number; end: number } }
}
export type PromptOptions = { model: ModelRef | null; agent: string; variant?: string; files?: PromptFile[] }
export type PromptSendResult = { ok: true } | { ok: false; error: string }
export type PermissionResponse = "once" | "always" | "reject"
export type ProviderAuthResult = { ok: boolean; connected: boolean }
export type SessionMoveResult = { ok: boolean; moved: string[]; error?: string }

type PermissionRequest = {
  id: string
  sessionID: string
  permission: string
  patterns?: string[]
  metadata?: Record<string, unknown>
  tool?: { messageID: string; callID: string }
}

// The engine caps an unspecified session-list limit at 100 rows, and the generated SDK's
// query type has no limit field, so the ceiling has to be forced past the typed shape.
export const sessionListLimit = 10000

export function sessionListQuery(directory?: string) {
  return { ...(directory === undefined ? {} : { directory }), limit: sessionListLimit } as { directory?: string }
}

// Proof of completeness, not a guess: a full page or a next cursor both mean rows were left behind.
export function sessionListComplete(sessions: readonly unknown[], cursor?: string | null) {
  return sessions.length < sessionListLimit && !cursor
}

function toPermission(request: PermissionRequest, directory: string): Permission {
  return {
    id: request.id,
    type: request.permission,
    pattern: request.patterns?.length ? [...request.patterns] : undefined,
    sessionID: request.sessionID,
    messageID: request.tool?.messageID ?? "",
    callID: request.tool?.callID,
    title: String(request.metadata?.title ?? request.permission),
    metadata: { ...request.metadata, directory },
    time: { created: Date.now() },
  }
}

export function createActions(
  requireClient: () => OpencodeClient,
  state: EngineState,
  set: SetStoreFunction<EngineState>,
  target: () => EngineTarget | undefined,
  recovery: RecoveryCoordinator = createRecoveryCoordinator(() => undefined),
) {
  const pageSize = 100
  let allSessionsRequest: { token: ReturnType<RecoveryCoordinator["begin"]>; promise: Promise<void> } | undefined
  const sessionRequests = new Map<string, { generation: number; promise: Promise<boolean> }>()

  const sessionEvents = (entry: BufferedEvent) => isSessionEvent(entry.event)
  const sessionEventsIn = (directory: string) => (entry: BufferedEvent) =>
    isSessionEvent(entry.event) && eventInDirectory(entry, directory)
  const transcriptEvents = (id: string) => (entry: BufferedEvent) =>
    isTranscriptEvent(entry.event) && sessionID(entry.event) === id

  function recordLinks(entries: { parts: { id: string }[] }[]) {
    for (const entry of entries) {
      for (const part of entry.parts as Parameters<typeof spawnLink>[0][]) {
        const link = spawnLink(part)
        if (!link) continue
        recordLink(link)
        set("links", link.child, link.parent)
      }
    }
  }

  async function requestTranscript(
    id: string,
    token: ReturnType<RecoveryCoordinator["begin"]>,
    loadedAtStart: boolean,
    lastAttempt: boolean,
  ) {
    try {
      const result = await requireClient().session.messages({
        path: { id },
        query: { limit: pageSize },
        signal: token.signal,
      })
      const entries = [...requireSdkData(result, "Could not load the session transcript")].sort((a, b) =>
        a.info.id.localeCompare(b.info.id),
      )
      let applied: ReturnType<typeof applyTranscriptSnapshot> | undefined
      const committed = recovery.commit(token, (events) => {
        applied = applyTranscriptSnapshot(
          state,
          set,
          id,
          entries,
          result.response?.headers?.get("x-next-cursor") ?? null,
          events,
          loadedAtStart,
          lastAttempt,
        )
      })
      if (applied === "applied") recordLinks(entries)
      return committed ? applied : undefined
    } finally {
      recovery.cancel(token)
    }
  }

  async function reloadSession(id: string) {
    const attempts = 3
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      const loadedAtStart = state.loaded[id] === true
      const token = recovery.begin(transcriptEvents(id))
      try {
        const result = await requestTranscript(id, token, loadedAtStart, attempt === attempts - 1)
        if (result === "applied") return true
        if (result !== "retry" || token.generation !== recovery.generation()) return false
      } catch (error) {
        if (token.generation !== recovery.generation() || !token.invalid || attempt > 1) throw error
      }
    }
    return false
  }

  // Older pages come via the raw route because the generated SDK lacks the cursor param.
  async function loadOlder(id: string) {
    const cursor = state.cursors[id]
    const base = target()
    if (!cursor || !base || !state.sessions[id]) return false
    const token = recovery.begin(transcriptEvents(id))
    const directory = state.directory
    const url =
      `${base.url}/session/${id}/message?directory=${encodeURIComponent(directory)}` +
      `&limit=${pageSize}&before=${encodeURIComponent(cursor)}`
    try {
      const response = await fetch(url, { headers: base.headers, signal: token.signal }).catch(() => null)
      if (!response?.ok) return false
      const older = ((await response.json().catch(() => [])) ?? []) as MessageEntry[]
      const sorted = [...older].sort((a, b) => a.info.id.localeCompare(b.info.id))
      let applied = false
      const committed = recovery.commit(token, () => {
        if (!state.sessions[id] || state.cursors[id] !== cursor || normalizeDir(state.directory) !== normalizeDir(directory))
          return
        set(
          produce((s) => {
            const existing = new Set((s.transcripts[id] ?? []).map((entry) => entry.info.id))
            s.transcripts[id] = [...sorted.filter((entry) => !existing.has(entry.info.id)), ...(s.transcripts[id] ?? [])]
            s.cursors[id] = response.headers.get("x-next-cursor")
          }),
        )
        applied = true
      })
      if (applied) recordLinks(sorted)
      return committed && applied && sorted.length > 0
    } finally {
      recovery.cancel(token)
    }
  }

  function openSession(id: string) {
    if (state.loaded[id]) return Promise.resolve(true)
    const current = sessionRequests.get(id)
    if (current && current.generation === recovery.generation()) return current.promise
    if (current) sessionRequests.delete(id)
    const generation = recovery.generation()
    let entry: { generation: number; promise: Promise<boolean> } | undefined
    const request = (async () => {
      set("loading", id, true)
      try {
        return await reloadSession(id)
      } catch (error) {
        if (generation === recovery.generation())
          notice({
            title: "Transcript load failed",
            message: error instanceof Error ? error.message : "Could not load the session transcript",
            variant: "error",
          })
        return false
      } finally {
        if (entry && sessionRequests.get(id) === entry)
          set(
            produce((s) => {
              delete s.loading[id]
            }),
          )
      }
    })()
    entry = { generation, promise: request }
    sessionRequests.set(id, entry)
    void request.finally(() => {
      if (entry && sessionRequests.get(id) === entry) sessionRequests.delete(id)
    })
    return request
  }

  async function loadSessions(directory: string) {
    const token = recovery.begin(sessionEventsIn(directory))
    try {
      const result = await requireClient().session.list({ query: sessionListQuery(directory), signal: token.signal })
      const sessions = requireSdkData(result, "Could not load sessions")
      recovery.commit(token, (events) =>
        applySessionSnapshot(
          set,
          sessions,
          sessionListComplete(sessions),
          (candidate) => normalizeDir(candidate) === normalizeDir(directory),
          events,
          recovery.replay,
        ),
      )
    } finally {
      recovery.cancel(token)
    }
  }

  // One DB query for every workspace. Avoids booting an OpenCode instance per project.
  async function loadAllSessions() {
    const base = target()
    if (!base) return
    if (allSessionsRequest && recovery.current(allSessionsRequest.token)) return allSessionsRequest.promise
    if (allSessionsRequest) allSessionsRequest = undefined
    const token = recovery.begin(sessionEvents)
    let entry: { token: typeof token; promise: Promise<void> }
    const request = (async () => {
      try {
        const query = new URLSearchParams({ archived: "true", limit: String(sessionListLimit) })
        const headers = {
          ...base.headers,
          ...(state.directory ? { "x-opencode-directory": encodeURIComponent(state.directory) } : {}),
        }
        const response = await fetch(`${base.url}/experimental/session?${query}`, {
          headers,
          signal: token.signal,
        }).catch(() => null)
        if (!response?.ok) return
        const sessions = (await response.json().catch(() => null)) as Session[] | null
        if (!Array.isArray(sessions)) return
        const complete = sessionListComplete(sessions, response.headers.get("x-next-cursor"))
        recovery.commit(token, (events) =>
          applySessionSnapshot(set, sessions, complete, () => true, events, recovery.replay),
        )
      } finally {
        recovery.cancel(token)
      }
    })()
    entry = { token, promise: request }
    allSessionsRequest = entry
    try {
      await request
    } finally {
      if (allSessionsRequest === entry) allSessionsRequest = undefined
    }
  }

  async function removeAllSessions(directory: string) {
    const result = await requireClient().session.list({ query: { directory } })
    for (const session of result.data ?? []) {
      await requireClient().session.delete({ path: { id: session.id }, query: { directory } })
    }
  }

  async function newSession(): Promise<(Session & { discard: () => Promise<void> }) | undefined> {
    const client = requireClient()
    const result = await client.session.create({ body: {} })
    const session = result.data
    if (!session) return
    set("sessions", session.id, session)
    return {
      ...session,
      async discard() {
        const result = await client.session.delete({ path: { id: session.id } }).catch(() => null)
        if (!result || result.error !== undefined) return
        forgetSession(session.id)
      },
    }
  }

  async function fork(id: string, mode: "active" | "full" = "active", messageID?: string): Promise<Session | undefined> {
    const base = target()
    if (!base) return
    return forkAt(id, mode, messageID, base, state.directory)
  }

  async function forkAt(
    id: string,
    mode: "active" | "full",
    messageID: string | undefined,
    base: EngineTarget,
    directory: string,
  ) {
    const response = await fetch(`${base.url}/session/${id}/fork?directory=${encodeURIComponent(directory)}`, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify({ mode, ...(messageID ? { messageID } : {}) }),
    }).catch(() => null)
    if (!response?.ok) {
      notice({ message: "The session could not be forked.", variant: "error" })
      return
    }
    const session = (await response.json().catch(() => undefined)) as Session | undefined
    if (session && normalizeDir(state.directory) === normalizeDir(directory)) putSession(set, session)
    return session
  }

  async function spawn(id: string, task: string, options: PromptOptions): Promise<Session | undefined> {
    const base = target()
    if (!base) return
    const directory = state.directory
    const client = createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory })
    const session = await forkAt(id, "active", undefined, base, directory)
    if (!session) return
    const title = task.trim().split(/\s+/).slice(0, 6).join(" ").slice(0, 64) || "Spawned thread"
    const renamed = await client.session.update({ path: { id: session.id }, body: { title } }).catch(() => null)
    if (!renamed || renamed.error) {
      await client.session.delete({ path: { id: session.id } }).catch(() => null)
      forgetSession(session.id)
      notice({ message: "The spawned thread could not be named.", variant: "error" })
      return
    }
    const body = {
      parts: [
        {
          type: "text" as const,
          text: [
            "This thread was spawned from another Drift conversation. Use the carried active context above.",
            "Start by forming a concise plan for the task, then carry it out.",
            `Task: ${task.trim()}`,
          ].join("\n\n"),
        },
      ],
      model: options.model ?? undefined,
      agent: options.agent,
      ...(options.variant ? { variant: options.variant } : {}),
    }
    const prompted = await client.session.promptAsync({ path: { id: session.id }, body }).catch(() => null)
    if (!prompted || prompted.error) {
      await client.session.delete({ path: { id: session.id } }).catch(() => null)
      forgetSession(session.id)
      notice({ message: "The spawned thread could not be started.", variant: "error" })
      return
    }
    const spawned = { ...session, title }
    if (normalizeDir(state.directory) === normalizeDir(directory)) putSession(set, spawned)
    const link = { child: session.id, parent: id }
    recordLink(link)
    set("links", link.child, link.parent)
    return spawned
  }

  async function sessionsAt(directory: string): Promise<Session[] | null> {
    const base = target()
    if (!base) return null
    const query = new URLSearchParams({ directory, archived: "true", limit: "10000" })
    const response = await fetch(`${base.url}/experimental/session?${query}`, { headers: base.headers }).catch(() => null)
    if (!response?.ok) return null
    return (await response.json().catch(() => null)) as Session[] | null
  }

  async function rebindSession(id: string, destination: string): Promise<string | null> {
    const base = target()
    if (!base) return "Engine is offline"
    const response = await fetch(`${base.url}/experimental/control-plane/move-session`, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify({ sessionID: id, destination: { directory: destination }, moveChanges: false }),
    }).catch(() => null)
    if (!response) return "Could not reach the engine"
    if (response.ok) return null
    const body = (await response.json().catch(() => null)) as { data?: { message?: string } } | null
    return body?.data?.message ?? `Engine rejected the move (${response.status})`
  }

  async function moveSessions(entries: Session[], destination: string): Promise<SessionMoveResult> {
    const moving = entries.filter((session) => normalizeDir(session.directory) !== normalizeDir(destination))
    if (!moving.length) return { ok: true, moved: [] }
    for (const session of moving) await interrupt(session.id)

    const completed: Session[] = []
    for (const session of moving) {
      const error = await rebindSession(session.id, destination)
      if (error) {
        for (const rollback of completed.reverse()) await rebindSession(rollback.id, rollback.directory)
        const restored = await sessionsAt(moving[0].directory)
        for (const info of restored ?? []) putSession(set, info)
        return { ok: false, moved: [], error }
      }
      completed.push(session)
      putSession(set, { ...session, directory: destination })
    }

    const refreshed = await sessionsAt(destination)
    const ids = new Set(moving.map((session) => session.id))
    for (const info of refreshed ?? []) if (ids.has(info.id)) putSession(set, info)
    return { ok: true, moved: moving.map((session) => session.id) }
  }

  async function moveSession(id: string, destination: string): Promise<SessionMoveResult> {
    const session = state.sessions[id]
    if (!session) return { ok: false, moved: [], error: "Session is no longer available" }
    const entries = await sessionsAt(session.directory)
    if (!entries) return { ok: false, moved: [], error: "Could not load the session tree" }
    return moveSessions(sessionTree(entries, id), destination)
  }

  async function moveWorkspaceSessions(source: string, destination: string): Promise<SessionMoveResult> {
    const entries = await sessionsAt(source)
    if (!entries) return { ok: false, moved: [], error: "Could not load sessions from this workspace" }
    return moveSessions(entries, destination)
  }

  async function send(id: string, text: string, options: PromptOptions): Promise<PromptSendResult> {
    set("errors", id, undefined!)
    const body = {
      parts: [
        ...(text.trim() ? [{ type: "text" as const, text }] : []),
        ...(options.files ?? []).map((file) => ({ type: "file" as const, ...file })),
      ],
      model: options.model ?? undefined,
      agent: options.agent,
      ...(options.variant ? { variant: options.variant } : {}),
    }
    if (body.parts.length === 0) {
      const error = "Prompt failed: the prompt is empty"
      set("errors", id, error)
      return { ok: false, error }
    }
    try {
      const result = await requireClient().session.promptAsync({ path: { id }, body })
      if (result.error !== undefined) {
        const error = `Prompt failed: ${sdkErrorMessage(result.error, "engine rejected the request")}`
        set("errors", id, error)
        return { ok: false, error }
      }
      return { ok: true }
    } catch (cause) {
      const error = `Prompt failed: ${sdkErrorMessage(cause, "could not reach the engine")}`
      set("errors", id, error)
      return { ok: false, error }
    }
  }

  async function abort(id: string) {
    await requireClient().session.abort({ path: { id } })
  }

  async function summarize(id: string, model: ModelRef | null) {
    if (!model) return
    await requireClient().session.summarize({ path: { id }, body: model })
  }

  async function share(id: string) {
    const result = await requireClient().session.share({ path: { id } })
    if (result.data) putSession(set, result.data)
    return result.data?.share?.url
  }

  async function unshare(id: string) {
    const result = await requireClient().session.unshare({ path: { id } })
    if (result.data) putSession(set, { ...result.data, share: undefined })
  }

  async function refreshProviders() {
    const token = recovery.begin(() => false)
    try {
      const result = await requireClient().provider.list({ signal: token.signal }).catch(() => null)
      if (!result?.data) return null
      const connected = result.data.connected ?? []
      const committed = recovery.commit(token, () => {
        set("providers", (result.data!.all ?? []) as unknown as ProviderInfo[])
        set("connected", connected)
        set("defaultModels", result.data!.default ?? {})
      })
      return committed ? connected : null
    } finally {
      recovery.cancel(token)
    }
  }

  async function refreshAgents() {
    const token = recovery.begin(() => false)
    try {
      const result = await requireClient().app.agents({ signal: token.signal }).catch(() => null)
      if (!result?.data) return false
      return recovery.commit(token, () => set("agents", result.data!))
    } finally {
      recovery.cancel(token)
    }
  }

  function controlClient() {
    const endpoint = target()
    if (!endpoint) return null
    return createControlClient({ baseUrl: endpoint.url, headers: endpoint.headers, directory: state.directory })
  }

  let reloadQueue = Promise.resolve(true)

  function reloadInstances() {
    const reload = async () => {
      const control = controlClient()
      if (!control) return false
      const result = await control.global.dispose().catch(() => null)
      if (result?.data !== true) return false
      await sleep(50)
      return true
    }
    reloadQueue = reloadQueue.then(reload, reload)
    return reloadQueue
  }

  async function syncProvider(id: string, changed: boolean): Promise<ProviderAuthResult> {
    if (!changed) return { ok: false, connected: state.connected.includes(id) }
    if (!(await reloadInstances())) return { ok: false, connected: state.connected.includes(id) }
    const connected = await refreshProviders()
    return { ok: connected !== null, connected: connected?.includes(id) ?? false }
  }

  async function providerAuthMethods() {
    const result = await requireClient().provider.auth().catch(() => null)
    return result?.data ?? {}
  }

  async function providerAuthorize(id: string, method: number) {
    const result = await requireClient().provider.oauth.authorize({ path: { id }, body: { method } })
    return result.data ?? null
  }

  async function providerCallback(id: string, method: number, code?: string) {
    const result = await requireClient()
      .provider.oauth.callback({ path: { id }, body: { method, ...(code ? { code } : {}) } })
      .catch(() => null)
    return syncProvider(id, result?.data === true)
  }

  async function setProviderKey(id: string, key: string) {
    const result = await requireClient()
      .auth.set({ path: { id }, body: { type: "api", key } })
      .catch(() => null)
    return syncProvider(id, result?.data === true)
  }

  async function disconnectProvider(id: string) {
    const control = controlClient()
    if (!control) return { ok: false, connected: state.connected.includes(id) }
    const result = await control.auth.remove({ providerID: id }).catch(() => null)
    return syncProvider(id, result?.data === true)
  }

  async function mcpStatus(directory = state.directory) {
    const result = await mcpClient(directory).mcp.status()
    return requireSdkData(result, "Could not load MCP status")
  }

  function mcpClient(directory: string) {
    if (normalizeDir(state.directory) !== normalizeDir(directory)) throw new Error("The active MCP workspace changed")
    return requireClient()
  }

  async function mcpConnect(name: string, directory = state.directory) {
    const result = await mcpClient(directory).mcp.connect({ path: { name } })
    if (requireSdkData(result, `Could not connect ${name}`) !== true) throw new Error(`Could not connect ${name}`)
  }

  async function mcpDisconnect(name: string, directory = state.directory) {
    const result = await mcpClient(directory).mcp.disconnect({ path: { name } })
    if (requireSdkData(result, `Could not disconnect ${name}`) !== true) throw new Error(`Could not disconnect ${name}`)
  }

  async function mcpAuthenticate(name: string, directory = state.directory) {
    const result = await mcpClient(directory).mcp.auth.authenticate({ path: { name } })
    requireSdkData(result, `Could not authenticate ${name}`)
  }

  async function findFiles(query: string) {
    const result = await requireClient().find.files({ query: { query } }).catch(() => null)
    return result?.data ?? []
  }

  async function runCommand(id: string, command: string, args: string) {
    await requireClient().session.command({ path: { id }, body: { command, arguments: args } })
  }

  async function rename(id: string, title: string) {
    await requireClient().session.update({ path: { id }, body: { title } })
  }

  async function remove(id: string) {
    await requireClient().session.delete({ path: { id } })
    forgetSession(id)
  }

  function forgetSession(id: string) {
    set(
      produce((s) => {
        dropSessionState(s, id)
      }),
    )
  }

  // Replied ids are filtered out of poll snapshots that raced the reply.
  const answered = new Set<string>()

  function askKey(kind: "permission" | "question", directory: string, id: string) {
    return `${kind}\0${normalizeDir(directory)}\0${id}`
  }

  type FetchResult<T> = { ok: true; data: T } | { ok: false }

  async function fetchJson<T>(path: string, dir: string, signal: AbortSignal): Promise<FetchResult<T>> {
    const base = target()
    if (!base) return { ok: false }
    const joiner = path.includes("?") ? "&" : "?"
    const response = await fetch(`${base.url}${path}${joiner}directory=${encodeURIComponent(dir)}`, {
      headers: base.headers,
      signal,
    }).catch(() => null)
    if (!response?.ok) return { ok: false }
    const data = (await response.json().catch(() => undefined)) as T | undefined
    return data === undefined ? { ok: false } : { ok: true, data }
  }

  // The generated SDK lags the engine here; GET /permission and /question recover asks
  // raised while we weren't listening. Walk directories one at a time so idle workspaces
  // don't stampede instance boots on a timer.
  const pendingDirectories = new Map<string, string>()
  let activeDirectories = new Map<string, string>()
  let permissionGeneration: number | undefined
  let permissionRequest: Promise<void> | undefined

  function refreshPermissions(directories: string[]) {
    if (!target()) return Promise.resolve()
    for (const directory of directories) {
      const key = normalizeDir(directory)
      if (!key) continue
      if (activeDirectories.has(key) && permissionGeneration === recovery.generation()) continue
      pendingDirectories.set(key, directory)
    }
    if (permissionRequest) return permissionRequest
    permissionRequest = drainPermissions().finally(() => {
      permissionRequest = undefined
    })
    return permissionRequest
  }

  async function drainPermissions() {
    try {
      while (pendingDirectories.size) {
        const directories = [...pendingDirectories.entries()]
        pendingDirectories.clear()
        const generation = recovery.generation()
        permissionGeneration = generation
        activeDirectories = new Map(directories)
        for (let index = 0; index < directories.length; index += 1) {
          const [key, directory] = directories[index]
          if (generation !== recovery.generation()) {
            for (const [pendingKey, pendingDirectory] of directories.slice(index))
              pendingDirectories.set(pendingKey, pendingDirectory)
            break
          }
          await refreshDirectoryAsks(directory)
          activeDirectories.delete(key)
          if (generation === recovery.generation()) continue
          for (const [pendingKey, pendingDirectory] of directories.slice(index))
            pendingDirectories.set(pendingKey, pendingDirectory)
          break
        }
      }
    } finally {
      activeDirectories = new Map()
      permissionGeneration = undefined
    }
  }

  async function refreshDirectoryAsks(directory: string) {
    const token = recovery.begin((entry) => {
      const type = (entry.event as { type: string }).type
      return (type.startsWith("permission.") || type.startsWith("question.")) && eventInDirectory(entry, directory)
    })
    try {
      const [permissionResult, questionResult] = await Promise.all([
        fetchJson<PermissionRequest[]>("/permission", directory, token.signal),
        fetchJson<QuestionRequest[]>("/question", directory, token.signal),
      ])
      const permissions = permissionResult.ok && Array.isArray(permissionResult.data)
        ? permissionResult.data.map((request) => toPermission(request, directory))
        : undefined
      const questions = questionResult.ok && Array.isArray(questionResult.data)
        ? questionResult.data.map((request) => ({ ...request, directory }))
        : undefined
      if (!permissions && !questions) return
      recovery.commit(token, (events) => reconcileAsks(directory, permissions, questions, events))
    } finally {
      recovery.cancel(token)
    }
  }

  function reconcileAsks(
    directory: string,
    permissions: Permission[] | undefined,
    questions: QuestionRequest[] | undefined,
    events: BufferedEvent[],
  ) {
    const touchedPermissions = touchedAsks(events, directory, "permission")
    const touchedQuestions = touchedAsks(events, directory, "question")
    set(
      produce((s) => {
        if (permissions) {
          for (const [sessionID, list] of Object.entries(s.permissions)) {
            s.permissions[sessionID] = list.filter((item) => {
              const dir = item.metadata?.directory
              return typeof dir !== "string" || normalizeDir(dir) !== normalizeDir(directory) || touchedPermissions.has(item.id)
            })
            if (!s.permissions[sessionID].length) delete s.permissions[sessionID]
          }
          for (const permission of permissions) {
            if (answered.has(askKey("permission", directory, permission.id)) || touchedPermissions.has(permission.id))
              continue
            const list = (s.permissions[permission.sessionID] ??= [])
            if (!list.some((item) => item.id === permission.id)) list.push(permission)
          }
        }
        if (questions) {
          for (const [sessionID, list] of Object.entries(s.questions)) {
            s.questions[sessionID] = list.filter(
              (item) => normalizeDir(item.directory ?? "") !== normalizeDir(directory) || touchedQuestions.has(item.id),
            )
            if (!s.questions[sessionID].length) delete s.questions[sessionID]
          }
          for (const question of questions) {
            if (answered.has(askKey("question", directory, question.id)) || touchedQuestions.has(question.id)) continue
            const list = (s.questions[question.sessionID] ??= [])
            if (!list.some((item) => item.id === question.id)) list.push(question)
          }
        }
      }),
    )
    if (permissions) {
      const reported = new Set(permissions.map((item) => askKey("permission", directory, item.id)))
      const prefix = askKey("permission", directory, "")
      for (const key of answered) if (key.startsWith(prefix) && !reported.has(key)) answered.delete(key)
    }
    if (questions) {
      const reported = new Set(questions.map((item) => askKey("question", directory, item.id)))
      const prefix = askKey("question", directory, "")
      for (const key of answered) if (key.startsWith(prefix) && !reported.has(key)) answered.delete(key)
    }
  }

  function touchedAsks(events: BufferedEvent[], directory: string, kind: "permission" | "question") {
    const ids = new Set<string>()
    for (const entry of events) {
      const raw = entry.event as unknown as { type: string; properties?: Record<string, unknown> }
      if (!raw.type.startsWith(`${kind}.`) || !eventInDirectory(entry, directory)) continue
      const id = raw.properties?.id ?? raw.properties?.requestID ?? raw.properties?.permissionID
      if (typeof id === "string") ids.add(id)
    }
    return ids
  }

  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    const permission = (state.permissions[sessionID] ?? []).find((p) => p.id === permissionID)
    const dir = permission?.metadata?.directory as string | undefined
    let ok = false
    try {
      if (dir && normalizeDir(dir) !== normalizeDir(state.directory)) {
        ok = await replyElsewhere(sessionID, permissionID, response, dir)
      } else {
        const result = await requireClient().postSessionIdPermissionsPermissionId({
          path: { id: sessionID, permissionID },
          body: { response },
        })
        ok = result.data === true
      }
    } catch {
      ok = false
    }
    if (!ok) {
      notice({ title: "Permission reply failed", message: "The permission is still pending. Try again.", variant: "error" })
      return false
    }
    answered.add(askKey("permission", dir ?? state.directory, permissionID))
    set(
      produce((s) => {
        s.permissions[sessionID] = (s.permissions[sessionID] ?? []).filter((p) => p.id !== permissionID)
      }),
    )
    return true
  }

  async function answerQuestion(sessionID: string, requestID: string, answers: string[][] | null) {
    const base = target()
    if (!base) return false
    const question = (state.questions[sessionID] ?? []).find((item) => item.id === requestID)
    const dir = question?.directory ?? state.directory
    const action = answers ? "reply" : "reject"
    const url = `${base.url}/question/${requestID}/${action}?directory=${encodeURIComponent(dir)}`
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify(answers ? { answers } : {}),
    }).catch(() => null)
    if (!response?.ok) return false
    answered.add(askKey("question", dir, requestID))
    set(
      produce((s) => {
        s.questions[sessionID] = (s.questions[sessionID] ?? []).filter((item) => item.id !== requestID)
      }),
    )
    return true
  }

  async function replyElsewhere(sessionID: string, permissionID: string, response: PermissionResponse, dir: string) {
    const base = target()
    if (!base) return false
    const url = `${base.url}/session/${sessionID}/permissions/${permissionID}?directory=${encodeURIComponent(dir)}`
    const result = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify({ response }),
    }).catch(() => null)
    return result?.ok === true
  }

  async function interrupt(id: string) {
    for (const permission of state.permissions[id] ?? [])
      await replyPermission(id, permission.id, "reject").catch(() => {})
    for (const question of state.questions[id] ?? []) await answerQuestion(id, question.id, null)
    if (!sessionBusy(state, id)) return
    await requireClient().session.abort({ path: { id } }).catch(() => {})
    for (let waited = 0; waited < 5000 && sessionBusy(state, id); waited += 100) await sleep(100)
  }

  async function revert(id: string, messageID: string) {
    await interrupt(id)
    const result = await requireClient().session.revert({ path: { id }, body: { messageID } })
    if (result.error) {
      set("errors", id, "Revert failed: the engine rejected the request")
      return false
    }
    set("errors", id, undefined!)
    if (result.data) putSession(set, result.data)
    await reloadSession(id)
    return true
  }

  async function unrevert(id: string) {
    await interrupt(id)
    const result = await requireClient().session.unrevert({ path: { id } })
    if (!result.data) return false
    putSession(set, result.data)
    await reloadSession(id)
    return true
  }

  function notice(
    input: Omit<Notice, "id" | "created" | "duration"> & { id?: string; created?: number; duration?: number },
  ) {
    pushNotice(set, {
      id: input.id ?? crypto.randomUUID(),
      duration: 8000,
      ...input,
      created: input.created ?? Date.now(),
    })
  }

  return {
    openSession,
    reloadSession,
    loadOlder,
    loadSessions,
    loadAllSessions,
    removeAllSessions,
    newSession,
    fork,
    spawn,
    moveSession,
    moveWorkspaceSessions,
    send,
    abort,
    summarize,
    share,
    unshare,
    findFiles,
    refreshProviders,
    refreshAgents,
    providerAuthMethods,
    providerAuthorize,
    providerCallback,
    setProviderKey,
    disconnectProvider,
    reloadInstances,
    mcpStatus,
    mcpConnect,
    mcpDisconnect,
    mcpAuthenticate,
    notice,
    runCommand,
    rename,
    remove,
    refreshPermissions,
    replyPermission,
    answerQuestion,
    revert,
    unrevert,
  }
}

export type EngineActions = ReturnType<typeof createActions>

export function requireSdkData<T>(result: { data?: T; error?: unknown }, fallback: string): T {
  if (result.error !== undefined) throw new Error(sdkErrorMessage(result.error, fallback))
  if (result.data === undefined) throw new Error(fallback)
  return result.data
}

function sdkErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message
  if (typeof error === "string" && error.trim()) return error
  if (error && typeof error === "object") {
    const record = error as Record<string, unknown>
    for (const key of ["message", "error", "data"]) {
      const value = record[key]
      if (typeof value === "string" && value.trim()) return value
      if (value && typeof value === "object") {
        const nested = sdkErrorMessage(value, "")
        if (nested) return nested
      }
    }
  }
  return fallback
}

export function sessionTree(sessions: Session[], rootId: string) {
  const ids = new Set([rootId])
  let changed = true
  while (changed) {
    changed = false
    for (const session of sessions) {
      if (!session.parentID || !ids.has(session.parentID) || ids.has(session.id)) continue
      ids.add(session.id)
      changed = true
    }
  }
  return sessions.filter((session) => ids.has(session.id))
}
