import { createOpencodeClient, type OpencodeClient, type Permission, type Session } from "@opencode-ai/sdk/client"
import { createOpencodeClient as createControlClient } from "@opencode-ai/sdk/v2/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { sleep, type EngineTarget } from "./connection"
import { pushNotice } from "./events"
import type { MessageEntry } from "./store"
import {
  normalizeDir,
  putSession,
  putSessions,
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
) {
  const pageSize = 100
  // The archive listing has no cursor, so it is fetched in one page with a ceiling high enough that
  // no real workspace reaches it.
  const archiveListLimit = "10000"
  // A spawned thread is titled from the first few words of its task.
  const titleWordCount = 6
  const maxTitleChars = 64
  // Disposing an instance is asynchronous on the engine side; this pause lets the old process release
  // its port and lock before a caller reconnects.
  const disposeSettleMs = 50
  // Aborting is best-effort: the engine may already be mid-turn. Poll until it reports idle, then give
  // up so the caller is never blocked indefinitely.
  const abortWaitMs = 5000
  const abortPollMs = 100
  const moveNoticeDurationMs = 8000
  let allSessionsRequest: Promise<void> | undefined

  // Writing `undefined` removes the key from the store. The non-null assertion is only there to
  // satisfy the setter's value type, which does not model deletion - it is not a real value.
  function clearSessionError(id: string) {
    set("errors", id, undefined!)
  }

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

  async function reloadSession(id: string) {
    const result = await requireClient().session.messages({ path: { id }, query: { limit: pageSize } })
    const entries = [...(result.data ?? [])].sort((a, b) => a.info.id.localeCompare(b.info.id))
    set("transcripts", id, entries)
    set("loaded", id, true)
    set("cursors", id, result.response?.headers?.get("x-next-cursor") ?? null)
    recordLinks(entries)
  }

  // Older pages come via the raw route because the generated SDK lacks the cursor param.
  async function loadOlder(id: string) {
    const cursor = state.cursors[id]
    const base = target()
    if (!cursor || !base) return false
    const url =
      withDirectory(`${base.url}/session/${id}/message`, state.directory) +
      `&limit=${pageSize}&before=${encodeURIComponent(cursor)}`
    const response = await engineFetch(url, { headers: base.headers })
    if (!response) return false
    const older = await readJson<MessageEntry[]>(response, [])
    const sorted = [...older].sort((a, b) => a.info.id.localeCompare(b.info.id))
    set(
      produce((draft) => {
        const existing = new Set((draft.transcripts[id] ?? []).map((entry) => entry.info.id))
        const added = sorted.filter((entry) => !existing.has(entry.info.id))
        draft.transcripts[id] = [...added, ...(draft.transcripts[id] ?? [])]
      }),
    )
    set("cursors", id, response.headers.get("x-next-cursor"))
    recordLinks(sorted)
    return sorted.length > 0
  }

  async function openSession(id: string) {
    if (state.loaded[id]) return
    set("loaded", id, true)
    await reloadSession(id)
  }

  async function loadSessions(directory: string) {
    const result = await requireClient().session.list({ query: { directory } })
    putSessions(set, result.data ?? [])
  }

  // One DB query for every workspace. Avoids booting an OpenCode instance per project.
  async function loadAllSessions() {
    const base = target()
    if (!base) return
    if (allSessionsRequest) return allSessionsRequest
    allSessionsRequest = (async () => {
      const query = new URLSearchParams({ archived: "true", limit: archiveListLimit })
      const headers = {
        ...base.headers,
        ...(state.directory ? { "x-opencode-directory": encodeURIComponent(state.directory) } : {}),
      }
      const response = await engineFetch(`${base.url}/experimental/session?${query}`, { headers })
      if (!response) return
      putSessions(set, await readJson<Session[]>(response, []))
    })()
    try {
      await allSessionsRequest
    } finally {
      allSessionsRequest = undefined
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
    const response = await engineFetch(withDirectory(`${base.url}/session/${id}/fork`, directory), {
      method: "POST",
      headers: jsonHeaders(base),
      body: JSON.stringify({ mode, ...(messageID ? { messageID } : {}) }),
    })
    if (!response) {
      notice({ message: "The session could not be forked.", variant: "error" })
      return
    }
    const session = await readJson<Session | undefined>(response, undefined)
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
    const title = task.trim().split(/\s+/).slice(0, titleWordCount).join(" ").slice(0, maxTitleChars) || "Spawned thread"
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
    const query = new URLSearchParams({ directory, archived: "true", limit: archiveListLimit })
    const response = await engineFetch(`${base.url}/experimental/session?${query}`, { headers: base.headers })
    return response ? readJson<Session[] | null>(response, null) : null
  }

  // Unlike the other raw routes this one reports why it failed, so it keeps the bare fetch: it has
  // to tell "could not reach the engine" apart from "the engine rejected the move" and read the
  // rejection body, which engineFetch deliberately collapses.
  async function rebindSession(id: string, destination: string): Promise<string | null> {
    const base = target()
    if (!base) return "Engine is offline"
    const response = await fetch(`${base.url}/experimental/control-plane/move-session`, {
      method: "POST",
      headers: jsonHeaders(base),
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
    clearSessionError(id)
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
    const result = await requireClient().provider.list().catch(() => null)
    if (!result?.data) return null
    set("providers", (result.data.all ?? []) as unknown as ProviderInfo[])
    set("connected", result.data.connected ?? [])
    set("defaultModels", result.data.default ?? {})
    return result.data.connected ?? []
  }

  async function refreshAgents() {
    const result = await requireClient().app.agents().catch(() => null)
    if (!result?.data) return false
    set("agents", result.data)
    return true
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
      await sleep(disposeSettleMs)
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

  // Drops only the transcript-shaped state for a session we deleted ourselves.
  // NOTE: events.ts dropSession, which handles the engine's own session-deleted event, additionally
  // clears permissions, questions, todos, status, activity and errors. The two have never agreed;
  // whether this one is missing those deletes has not been established.
  function forgetSession(id: string) {
    set(
      produce((draft) => {
        delete draft.sessions[id]
        delete draft.transcripts[id]
        delete draft.loaded[id]
      }),
    )
  }

  // Replied ids are filtered out of poll snapshots that raced the reply.
  const answered = new Set<string>()

  async function fetchJson<T>(path: string, dir: string): Promise<T | null> {
    const base = target()
    if (!base) return null
    const response = await engineFetch(withDirectory(`${base.url}${path}`, dir), { headers: base.headers })
    return response ? readJson<T | null>(response, null) : null
  }

  // The generated SDK lags the engine here; GET /permission and /question recover asks
  // raised while we weren't listening. Walk directories one at a time so idle workspaces
  // don't stampede instance boots on a timer.
  async function refreshPermissions(directories: string[]) {
    if (!target() || directories.length === 0) return
    const results: {
      permissions: Permission[]
      questions: QuestionRequest[]
    }[] = []
    for (const dir of directories) {
      const [permissions, questions] = await Promise.all([
        fetchJson<PermissionRequest[]>("/permission", dir),
        fetchJson<QuestionRequest[]>("/question", dir),
      ])
      results.push({
        permissions: (permissions ?? []).map((request) => toPermission(request, dir)),
        questions: (questions ?? []).map((request) => ({ ...request, directory: dir })),
      })
    }
    const keep = new Set(directories.map(normalizeDir))
    const reported = new Set(results.flatMap((r) => [...r.permissions, ...r.questions].map((item) => item.id)))
    const previous = new Set<string>()
    for (const list of Object.values(state.permissions)) {
      for (const item of list) {
        const dir = item.metadata?.directory
        if (typeof dir === "string" && keep.has(normalizeDir(dir))) previous.add(item.id)
      }
    }
    for (const list of Object.values(state.questions)) {
      for (const item of list) {
        if (item.directory && keep.has(normalizeDir(item.directory))) previous.add(item.id)
      }
    }
    for (const id of previous) if (!reported.has(id)) answered.delete(id)
    set(
      produce((draft) => {
        for (const [sessionID, list] of Object.entries(draft.permissions)) {
          draft.permissions[sessionID] = list.filter((item) => {
            const dir = item.metadata?.directory
            return typeof dir === "string" && !keep.has(normalizeDir(dir))
          })
          if (!draft.permissions[sessionID]?.length) delete draft.permissions[sessionID]
        }
        for (const [sessionID, list] of Object.entries(draft.questions)) {
          draft.questions[sessionID] = list.filter((item) => {
            const dir = item.directory
            return typeof dir === "string" && !keep.has(normalizeDir(dir))
          })
          if (!draft.questions[sessionID]?.length) delete draft.questions[sessionID]
        }
        for (const result of results) {
          for (const permission of result.permissions)
            if (!answered.has(permission.id)) (draft.permissions[permission.sessionID] ??= []).push(permission)
          for (const question of result.questions)
            if (!answered.has(question.id)) (draft.questions[question.sessionID] ??= []).push(question)
        }
      }),
    )
  }

  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    const permission = (state.permissions[sessionID] ?? []).find((p) => p.id === permissionID)
    const dir = permission?.metadata?.directory as string | undefined
    const failed = () => {
      const message = "The request is still pending. Try again."
      notice({ title: "Permission reply failed", message, variant: "error" })
      return false
    }
    if (dir && normalizeDir(dir) !== normalizeDir(state.directory)) {
      // A swallowed failure would hide the card while the engine stays blocked on the ask, and
      // because the card is gone poll reconciliation never clears it from `answered`.
      if (!(await replyElsewhere(sessionID, permissionID, response, dir))) return failed()
    } else {
      // requireClient() throws while offline and the request itself can reject. UI callers
      // fire-and-forget this action, so both must resolve to a visible failure.
      try {
        const result = await requireClient().postSessionIdPermissionsPermissionId({
          path: { id: sessionID, permissionID },
          body: { response },
        })
        if (result.error) return failed()
      } catch {
        return failed()
      }
    }
    answered.add(permissionID)
    set(
      produce((draft) => {
        draft.permissions[sessionID] = (draft.permissions[sessionID] ?? []).filter((p) => p.id !== permissionID)
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
    const url = withDirectory(`${base.url}/question/${requestID}/${action}`, dir)
    const response = await engineFetch(url, {
      method: "POST",
      headers: jsonHeaders(base),
      body: JSON.stringify(answers ? { answers } : {}),
    })
    if (!response) return false
    answered.add(requestID)
    set(
      produce((draft) => {
        draft.questions[sessionID] = (draft.questions[sessionID] ?? []).filter((item) => item.id !== requestID)
      }),
    )
    return true
  }

  async function replyElsewhere(sessionID: string, permissionID: string, response: PermissionResponse, dir: string) {
    const base = target()
    if (!base) return false
    const url = withDirectory(`${base.url}/session/${sessionID}/permissions/${permissionID}`, dir)
    const result = await engineFetch(url, {
      method: "POST",
      headers: jsonHeaders(base),
      body: JSON.stringify({ response }),
    })
    return result !== null
  }

  async function interrupt(id: string) {
    for (const permission of state.permissions[id] ?? [])
      await replyPermission(id, permission.id, "reject").catch(() => {})
    for (const question of state.questions[id] ?? []) await answerQuestion(id, question.id, null)
    if (!sessionBusy(state, id)) return
    await requireClient().session.abort({ path: { id } }).catch(() => {})
    for (let waited = 0; waited < abortWaitMs && sessionBusy(state, id); waited += abortPollMs) await sleep(abortPollMs)
  }

  async function revert(id: string, messageID: string) {
    await interrupt(id)
    const result = await requireClient().session.revert({ path: { id }, body: { messageID } })
    if (result.error) {
      set("errors", id, "Revert failed: the engine rejected the request")
      return false
    }
    clearSessionError(id)
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
      duration: moveNoticeDurationMs,
      ...input,
      created: input.created ?? Date.now(),
    })
  }

  return {
    openSession,
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

/**
 * Engine routes are per-directory: the server uses this parameter to pick which instance handles
 * the request. Every raw route below needs it, so it is applied in one place.
 */
function withDirectory(url: string, directory: string) {
  const joiner = url.includes("?") ? "&" : "?"
  return `${url}${joiner}directory=${encodeURIComponent(directory)}`
}

function jsonHeaders(base: EngineTarget) {
  return { "content-type": "application/json", ...base.headers }
}

/**
 * A request against a raw engine route. Resolves to the response only when the request completed
 * and the engine accepted it; a transport failure and a non-2xx both collapse to null, because
 * every caller treats them the same way.
 */
async function engineFetch(url: string, init?: RequestInit): Promise<Response | null> {
  const response = await fetch(url, init).catch(() => null)
  return response?.ok ? response : null
}

/** Reads a JSON body, falling back rather than throwing if the payload is absent or truncated. */
async function readJson<T>(response: Response, fallback: T): Promise<T> {
  return ((await response.json().catch(() => fallback)) ?? fallback) as T
}

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
