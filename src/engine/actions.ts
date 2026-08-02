import { createOpencodeClient, type OpencodeClient, type Permission, type Session } from "@opencode-ai/sdk/client"
import { createOpencodeClient as createControlClient } from "@opencode-ai/sdk/v2/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { t } from "../state/i18n"
import { clearRecoverableInterruption, updateRecoverableFailure } from "../state/recovery"
import {
  beginPermissionReply,
  clearPermissionAttention,
  clearPermissionAttentionFor,
  failPermissionReply,
  observePermission,
  prunePermissionAttention,
  type DriftPermission,
} from "../state/permission-attention"
import { sleep, type EngineTarget } from "./connection"
import { applySessionSnapshot, purgeSession, pushNotice } from "./events"
import type { MessageEntry } from "./store"
import {
  askRevision,
  bumpAskRevision,
  captureRevisions,
  interruptStaleTools,
  mergeTranscriptSnapshot,
  normalizeDir,
  putSession,
  recordLink,
  sessionBusy,
  sessionSnapshotLimit,
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

export const RECOVERY_INSTRUCTION = [
  "A recoverable model or provider failure interrupted this session.",
  "Reassess the durable transcript, completed tool results, current todos, and workspace state before continuing.",
  "Continue the existing task from the latest durable state. Do not blindly repeat tools or work that already succeeded.",
].join(" ")

type PermissionRequest = {
  id: string
  sessionID: string
  permission: string
  patterns?: string[]
  always?: string[]
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
    metadata: { ...request.metadata, ...(request.always ? { always: request.always } : {}), directory },
    time: { created: Date.now() },
  }
}

const defaultAbortWait = { waitMs: 5000, pollMs: 100 }

export function createActions(
  requireClient: () => OpencodeClient,
  state: EngineState,
  set: SetStoreFunction<EngineState>,
  target: () => EngineTarget | undefined,
  abortWait: { waitMs: number; pollMs: number } = defaultAbortWait,
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
  const moveNoticeDurationMs = 8000
  const askPollTimeoutMs = 8000
  const permissionReplyTimeoutMs = 8000
  let allSessionsRequest: Promise<void> | undefined
  const transcriptRequests = new Map<string, Promise<boolean>>()
  const permissionReplies = new Map<string, Promise<boolean>>()
  const queuedAskDirectories = new Map<string, string>()
  const activeAskDirectories = new Set<string>()
  let askRefresh: Promise<void> | undefined

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
    const captured = captureRevisions(state)
    const existed = id in state.sessions
    const result = await requireClient().session.messages({ path: { id }, query: { limit: pageSize } })
    const entries = interruptStaleTools(
      [...requireSdkData(result, "Could not load transcript")].sort((a, b) => a.info.id.localeCompare(b.info.id)),
      state.liveTools,
      t("drift.message.interrupted"),
    )
    // The session vanished while the transcript was in flight; applying would resurrect it.
    if (existed && !state.sessions[id]) return
    set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], entries, id, captured, state.revisions))
    set("loaded", id, true)
    set("cursors", id, result.response?.headers?.get("x-next-cursor") ?? null)
    recordLinks(entries)
    const latest = [...entries].reverse().find((entry) => entry.info.role === "assistant")?.info
    if (latest?.role === "assistant" && latest.time.completed && !latest.error)
      clearRecoverableInterruption(id, true)
  }

  function reportTranscriptFailure(id: string, cause: unknown) {
    notice({
      id: `transcript-load-${id}`,
      title: "Transcript load failed",
      message: sdkErrorMessage(cause, "Could not reach the engine"),
      variant: "error",
    })
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
    const sorted = interruptStaleTools(
      [...older].sort((a, b) => a.info.id.localeCompare(b.info.id)),
      state.liveTools,
      t("drift.message.interrupted"),
    )
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

  function openSession(id: string) {
    if (state.loaded[id]) return Promise.resolve(true)
    const active = transcriptRequests.get(id)
    if (active) return active
    let request!: Promise<boolean>
    request = reloadSession(id)
      .then(() => true)
      .catch((cause) => {
        reportTranscriptFailure(id, cause)
        return false
      })
      .finally(() => {
        if (transcriptRequests.get(id) === request) transcriptRequests.delete(id)
      })
    transcriptRequests.set(id, request)
    return request
  }

  async function loadSessions(directory: string) {
    const captured = captureRevisions(state)
    const result = await requireClient().session.list({ query: { directory } })
    const sessions = result.data
    if (result.error !== undefined || !sessions) return
    const complete = sessions.length < sessionSnapshotLimit
    applySessionSnapshot(set, { sessions, captured, ...(complete ? { scope: { directory } } : {}) })
  }

  // One DB query for every workspace. Avoids booting an OpenCode instance per project.
  async function loadAllSessions() {
    const base = target()
    if (!base) return
    if (allSessionsRequest) return allSessionsRequest
    allSessionsRequest = (async () => {
      const captured = captureRevisions(state)
      const query = new URLSearchParams({ archived: "true", limit: archiveListLimit })
      const headers = {
        ...base.headers,
        ...(state.directory ? { "x-opencode-directory": encodeURIComponent(state.directory) } : {}),
      }
      const response = await engineFetch(`${base.url}/experimental/session?${query}`, { headers })
      if (!response) return
      applySessionSnapshot(set, { sessions: await readJson<Session[]>(response, []), captured })
    })()
    try {
      await allSessionsRequest
    } finally {
      allSessionsRequest = undefined
    }
  }

  // Drains in passes because the engine caps each listing at 100. False means the directory
  // wasn't fully drained, so callers keep their bookkeeping and retry on a later purge.
  // `eligible` is re-checked every pass so a workspace restored mid-drain stops the sweep.
  async function removeAllSessions(directory: string, eligible: () => boolean = () => true): Promise<boolean> {
    const attempted = new Set<string>()
    for (;;) {
      if (!eligible()) return false
      const result = await requireClient()
        .session.list({ query: { directory } })
        .catch(() => null)
      if (!result || result.error !== undefined) return false
      const sessions = result.data ?? []
      if (!sessions.length) return true
      // Every remaining session already survived a delete attempt: the drain is stuck.
      if (sessions.every((session) => attempted.has(session.id))) return false
      for (const session of sessions) {
        attempted.add(session.id)
        await requireClient()
          .session.delete({ path: { id: session.id }, query: { directory } })
          .catch(() => null)
      }
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
          metadata: { generated: true },
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
    for (const session of moving) {
      if (!(await interrupt(session.id))) return { ok: false, moved: [], error: t("drift.move.sessionBusy") }
    }

    const completed: Session[] = []
    for (const session of moving) {
      const error = await rebindSession(session.id, destination)
      if (error) {
        const stillMoved: string[] = []
        let rollbackError: string | undefined
        for (const rollback of completed.reverse()) {
          const failure = await rebindSession(rollback.id, rollback.directory)
          if (failure) {
            rollbackError ??= failure
            stillMoved.push(rollback.id)
            continue
          }
          putSession(set, rollback)
        }
        return {
          ok: false,
          moved: stillMoved,
          error: rollbackError ? `${error}; rollback failed: ${rollbackError}` : error,
        }
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
      const base = target()
      const directory = state.sessions[id]?.directory
      const client = base && directory
        ? createOpencodeClient({ baseUrl: base.url, headers: base.headers, directory })
        : requireClient()
      const result = await client.session.promptAsync({ path: { id }, body })
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

  async function recover(id: string, options: PromptOptions): Promise<PromptSendResult> {
    if (!options.model) return { ok: false, error: "Recovery requires a model" }
    clearSessionError(id)
    const body = {
      parts: [{ type: "text" as const, text: RECOVERY_INSTRUCTION, metadata: { generated: true } }],
      model: options.model,
      agent: options.agent,
      ...(options.variant ? { variant: options.variant } : {}),
    }
    try {
      const result = await requireClient().session.promptAsync({ path: { id }, body })
      if (result.error === undefined) return { ok: true }
      const error = `Recovery failed: ${sdkErrorMessage(result.error, "engine rejected the request")}`
      set("errors", id, error)
      updateRecoverableFailure(id, error, options.model)
      return { ok: false, error }
    } catch (cause) {
      const error = `Recovery failed: ${sdkErrorMessage(cause, "could not reach the engine")}`
      set("errors", id, error)
      updateRecoverableFailure(id, error, options.model)
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

  // Reloads are serialized: two overlapping disposes would race to restart the same instance.
  let reloadQueue = Promise.resolve(true)

  function reloadInstances() {
    const reload = async () => {
      const control = controlClient()
      if (!control) return false
      const result = await control.global.dispose().catch(() => null)
      if (result?.data !== true) return false
      clearPermissionAttentionFor(Object.values(state.permissions).flat())
      set("liveTools", {})
      await sleep(disposeSettleMs)
      return true
    }
    // `reload` is passed as both handlers so it runs whether the previous reload resolved or
    // rejected - a failed reload must not block every reload after it.
    reloadQueue = reloadQueue.then(reload, reload)
    return reloadQueue
  }

  async function reloadProviders() {
    if (!(await reloadInstances())) return false
    return (await refreshProviders()) !== null
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

  // Purge deletions must be confirmed before Drift drops its tombstone, so unlike `remove` this
  // validates the SDK result. A 404 means the engine already lost the session (e.g. a previous
  // purge deleted it before the tombstone was cleared), which counts as removed.
  async function purgeSession(id: string): Promise<boolean> {
    try {
      const result = await requireClient().session.delete({ path: { id } })
      if (result.error !== undefined && result.response.status !== 404) return false
      forgetSession(id)
      return true
    } catch {
      return false
    }
  }

  function forgetSession(id: string) {
    set(produce((draft) => purgeSession(draft, id)))
  }

  // Replied ids are filtered out of poll snapshots that raced the reply.
  const answered = new Set<string>()

  function askKey(kind: "permission" | "question", dir: string, id: string) {
    return `${kind}\0${normalizeDir(dir)}\0${id}`
  }

  async function fetchAskSnapshot<T>(path: string, dir: string): Promise<{ ok: true; data: T[] } | { ok: false }> {
    const base = target()
    if (!base) return { ok: false }
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), askPollTimeoutMs)
    try {
      const response = await fetch(withDirectory(`${base.url}${path}`, dir), {
        headers: base.headers,
        signal: controller.signal,
      }).catch(() => null)
      if (!response?.ok) return { ok: false }
      const data = await response.json().catch(() => null)
      return Array.isArray(data) ? { ok: true, data: data as T[] } : { ok: false }
    } finally {
      clearTimeout(timeout)
    }
  }

  function clearConfirmedAnswers(kind: "permission" | "question", dir: string, reported: Set<string>) {
    const prefix = `${kind}\0${normalizeDir(dir)}\0`
    for (const key of answered) {
      if (!key.startsWith(prefix)) continue
      if (!reported.has(key.slice(prefix.length))) answered.delete(key)
    }
  }

  function reconcilePermissions(dir: string, permissions: Permission[]) {
    const reported = new Set(permissions.map((permission) => permission.id))
    clearConfirmedAnswers("permission", dir, reported)
    const normalized = normalizeDir(dir)
    for (const permission of permissions) observePermission(permission, state)
    set(
      produce((draft) => {
        for (const [sessionID, list] of Object.entries(draft.permissions)) {
          draft.permissions[sessionID] = list.filter((permission) => {
            if ((permission as DriftPermission).driftProtocol === "v2") return true
            const directory = permission.metadata?.directory
            return typeof directory !== "string" || normalizeDir(directory) !== normalized
          })
          if (!draft.permissions[sessionID]?.length) delete draft.permissions[sessionID]
        }
        for (const permission of permissions) {
          if (answered.has(askKey("permission", dir, permission.id))) continue
          const list = (draft.permissions[permission.sessionID] ??= [])
          if (!list.some((current) => current.id === permission.id)) list.push(permission)
        }
      }),
    )
    prunePermissionAttention(new Set(Object.values(state.permissions).flat().map((permission) => permission.id)))
  }

  function reconcileQuestions(dir: string, questions: QuestionRequest[]) {
    const reported = new Set(questions.map((question) => question.id))
    clearConfirmedAnswers("question", dir, reported)
    const normalized = normalizeDir(dir)
    set(
      produce((draft) => {
        for (const [sessionID, list] of Object.entries(draft.questions)) {
          draft.questions[sessionID] = list.filter(
            (question) => !question.directory || normalizeDir(question.directory) !== normalized,
          )
          if (!draft.questions[sessionID]?.length) delete draft.questions[sessionID]
        }
        for (const question of questions) {
          if (answered.has(askKey("question", dir, question.id))) continue
          const list = (draft.questions[question.sessionID] ??= [])
          if (!list.some((current) => current.id === question.id)) list.push(question)
        }
      }),
    )
  }

  async function refreshDirectoryAsks(dir: string) {
    const permissionRevision = askRevision(state, "permission", dir)
    const questionRevision = askRevision(state, "question", dir)
    const [permissionResult, questionResult] = await Promise.all([
      fetchAskSnapshot<PermissionRequest>("/permission", dir),
      fetchAskSnapshot<QuestionRequest>("/question", dir),
    ])
    if (permissionResult.ok && askRevision(state, "permission", dir) === permissionRevision) {
      reconcilePermissions(
        dir,
        permissionResult.data.map((request) => toPermission(request, dir)),
      )
    }
    if (questionResult.ok && askRevision(state, "question", dir) === questionRevision) {
      reconcileQuestions(
        dir,
        questionResult.data.map((request) => ({ ...request, directory: dir })),
      )
    }
  }

  async function drainAskRefresh() {
    while (queuedAskDirectories.size) {
      const next = queuedAskDirectories.entries().next().value as [string, string]
      queuedAskDirectories.delete(next[0])
      activeAskDirectories.add(next[0])
      try {
        await refreshDirectoryAsks(next[1])
      } finally {
        activeAskDirectories.delete(next[0])
      }
    }
  }

  // The generated SDK lags the engine here; GET /permission and /question recover asks
  // raised while we weren't listening. Polls are coalesced and directories are walked one
  // at a time so idle workspaces do not stampede instance boots.
  function refreshPermissions(directories: string[]) {
    if (!target()) return Promise.resolve()
    for (const dir of directories) {
      const normalized = normalizeDir(dir)
      if (!normalized || activeAskDirectories.has(normalized) || queuedAskDirectories.has(normalized)) continue
      queuedAskDirectories.set(normalized, dir)
    }
    if (askRefresh) return askRefresh
    let request!: Promise<void>
    request = drainAskRefresh().finally(() => {
      if (askRefresh === request) askRefresh = undefined
    })
    askRefresh = request
    return request
  }

  function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    const key = `${sessionID}\0${permissionID}`
    const existing = permissionReplies.get(key)
    if (existing) return existing
    const reply = sendPermissionReply(sessionID, permissionID, response).finally(() => permissionReplies.delete(key))
    permissionReplies.set(key, reply)
    return reply
  }

  async function sendPermissionReply(sessionID: string, permissionID: string, response: PermissionResponse) {
    const permission = (state.permissions[sessionID] ?? []).find((p) => p.id === permissionID)
    if (permission) beginPermissionReply(permission, response, Object.values(state.permissions).flat())
    const dir = permission?.metadata?.directory as string | undefined
    const failed = () => {
      failPermissionReply(permissionID)
      const message = "The request is still pending. Try again."
      notice({ title: "Permission reply failed", message, variant: "error" })
      return false
    }
    const answerDirectory = dir ?? state.directory
    if (!(await postPermissionReply(permission, sessionID, permissionID, response, answerDirectory))) return failed()
    answered.add(askKey("permission", answerDirectory, permissionID))
    set(
      produce((draft) => {
        draft.permissions[sessionID] = (draft.permissions[sessionID] ?? []).filter((p) => p.id !== permissionID)
        bumpAskRevision(draft, "permission", answerDirectory)
      }),
    )
    clearPermissionAttention(permissionID)
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
    answered.add(askKey("question", dir, requestID))
    set(
      produce((draft) => {
        draft.questions[sessionID] = (draft.questions[sessionID] ?? []).filter((item) => item.id !== requestID)
        bumpAskRevision(draft, "question", dir)
      }),
    )
    return true
  }

  async function postPermissionReply(
    permission: Permission | undefined,
    sessionID: string,
    permissionID: string,
    response: PermissionResponse,
    dir: string,
  ) {
    const base = target()
    if (!base) return false
    const v2 = (permission as DriftPermission | undefined)?.driftProtocol === "v2"
    const path = v2
      ? `/api/session/${sessionID}/permission/${permissionID}/reply`
      : `/session/${sessionID}/permissions/${permissionID}`
    const url = withDirectory(`${base.url}${path}`, dir)
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), permissionReplyTimeoutMs)
    try {
      const result = await engineFetch(url, {
        method: "POST",
        headers: jsonHeaders(base),
        body: JSON.stringify(v2 ? { reply: response } : { response }),
        signal: controller.signal,
      })
      return result !== null
    } finally {
      clearTimeout(timeout)
    }
  }

  // Resolves true only when the session is confirmed idle after the abort settled.
  async function interrupt(id: string): Promise<boolean> {
    for (const permission of state.permissions[id] ?? [])
      await replyPermission(id, permission.id, "reject").catch(() => {})
    for (const question of state.questions[id] ?? []) await answerQuestion(id, question.id, null)
    if (!sessionBusy(state, id)) return true
    await requireClient().session.abort({ path: { id } }).catch(() => {})
    for (let waited = 0; waited < abortWait.waitMs && sessionBusy(state, id); waited += abortWait.pollMs)
      await sleep(abortWait.pollMs)
    return !sessionBusy(state, id)
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
    await reloadSession(id).catch((cause) => reportTranscriptFailure(id, cause))
    return true
  }

  async function unrevert(id: string) {
    await interrupt(id)
    const result = await requireClient().session.unrevert({ path: { id } })
    if (!result.data) return false
    putSession(set, result.data)
    await reloadSession(id).catch((cause) => reportTranscriptFailure(id, cause))
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
    recover,
    abort,
    summarize,
    share,
    unshare,
    findFiles,
    refreshProviders,
    reloadProviders,
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
    purgeSession,
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
  const byParent = new Map<string, Session[]>()
  for (const session of sessions) {
    if (!session.parentID) continue
    const children = byParent.get(session.parentID) ?? []
    children.push(session)
    byParent.set(session.parentID, children)
  }
  const root = sessions.find((session) => session.id === rootId)
  if (!root) return []
  const result = [root]
  const seen = new Set([root.id])
  for (let index = 0; index < result.length; index++) {
    for (const child of byParent.get(result[index].id) ?? []) {
      if (seen.has(child.id)) continue
      seen.add(child.id)
      result.push(child)
    }
  }
  return result
}
