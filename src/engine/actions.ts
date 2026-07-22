import type { OpencodeClient, Permission, Session } from "@opencode-ai/sdk/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { sleep, type EngineTarget } from "./connection"
import type { MessageEntry } from "./store"
import {
  normalizeDir,
  putSession,
  recordLink,
  sessionBusy,
  spawnLink,
  type EngineState,
  type ModelRef,
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
export type PermissionResponse = "once" | "always" | "reject"

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
      `${base.url}/session/${id}/message?directory=${encodeURIComponent(state.directory)}` +
      `&limit=${pageSize}&before=${encodeURIComponent(cursor)}`
    const response = await fetch(url, { headers: base.headers }).catch(() => null)
    if (!response?.ok) return false
    const older = ((await response.json().catch(() => [])) ?? []) as MessageEntry[]
    const sorted = [...older].sort((a, b) => a.info.id.localeCompare(b.info.id))
    set(
      produce((s) => {
        const existing = new Set((s.transcripts[id] ?? []).map((entry) => entry.info.id))
        s.transcripts[id] = [...sorted.filter((entry) => !existing.has(entry.info.id)), ...(s.transcripts[id] ?? [])]
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
    for (const session of result.data ?? []) putSession(set, session)
  }

  async function removeAllSessions(directory: string) {
    const result = await requireClient().session.list({ query: { directory } })
    for (const session of result.data ?? []) {
      await requireClient().session.delete({ path: { id: session.id }, query: { directory } })
    }
  }

  async function newSession(): Promise<Session | undefined> {
    const result = await requireClient().session.create({ body: {} })
    if (result.data) set("sessions", result.data.id, result.data)
    return result.data
  }

  async function fork(id: string): Promise<Session | undefined> {
    const result = await requireClient().session.fork({ path: { id }, body: {} })
    if (result.data) set("sessions", result.data.id, result.data)
    return result.data
  }

  async function send(id: string, text: string, options: PromptOptions) {
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
    if (body.parts.length === 0) return
    const result = await requireClient().session.promptAsync({ path: { id }, body })
    if (result.error) set("errors", id, "Prompt failed: engine rejected the request")
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
    if (!result?.data) return
    set("providers", (result.data.all ?? []) as unknown as ProviderInfo[])
    set("connected", result.data.connected ?? [])
    set("defaultModels", result.data.default ?? {})
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
    return result?.data === true
  }

  async function setProviderKey(id: string, key: string) {
    const result = await requireClient()
      .auth.set({ path: { id }, body: { type: "api", key } })
      .catch(() => null)
    return result?.data === true
  }

  async function mcpStatus() {
    const result = await requireClient().mcp.status().catch(() => null)
    return result?.data ?? {}
  }

  async function mcpConnect(name: string) {
    await requireClient().mcp.connect({ path: { name } }).catch(() => {})
  }

  async function mcpDisconnect(name: string) {
    await requireClient().mcp.disconnect({ path: { name } }).catch(() => {})
  }

  async function mcpAuthenticate(name: string) {
    await requireClient().mcp.auth.authenticate({ path: { name } }).catch(() => {})
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
    set(
      produce((s) => {
        delete s.sessions[id]
        delete s.transcripts[id]
        delete s.loaded[id]
      }),
    )
  }

  // Replied ids are filtered out of poll snapshots that raced the reply.
  const answered = new Set<string>()

  async function fetchJson<T>(path: string, dir: string): Promise<T | null> {
    const base = target()
    if (!base) return null
    const joiner = path.includes("?") ? "&" : "?"
    const response = await fetch(`${base.url}${path}${joiner}directory=${encodeURIComponent(dir)}`, {
      headers: base.headers,
    }).catch(() => null)
    if (!response?.ok) return null
    return (await response.json().catch(() => null)) as T | null
  }

  // The generated SDK lags the engine here; GET /permission and /question recover asks
  // raised while we weren't listening, across every workspace directory.
  async function refreshPermissions(directories: string[]) {
    if (!target()) return
    const results = await Promise.all(
      directories.map(async (dir) => {
        const [permissions, questions] = await Promise.all([
          fetchJson<PermissionRequest[]>("/permission", dir),
          fetchJson<QuestionRequest[]>("/question", dir),
        ])
        return {
          permissions: (permissions ?? []).map((request) => toPermission(request, dir)),
          questions: (questions ?? []).map((request) => ({ ...request, directory: dir })),
        }
      }),
    )
    const reported = new Set(results.flatMap((r) => [...r.permissions, ...r.questions].map((item) => item.id)))
    for (const id of answered) if (!reported.has(id)) answered.delete(id)
    set(
      produce((s) => {
        s.permissions = {}
        s.questions = {}
        for (const result of results) {
          for (const permission of result.permissions)
            if (!answered.has(permission.id)) (s.permissions[permission.sessionID] ??= []).push(permission)
          for (const question of result.questions)
            if (!answered.has(question.id)) (s.questions[question.sessionID] ??= []).push(question)
        }
      }),
    )
  }

  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    const permission = (state.permissions[sessionID] ?? []).find((p) => p.id === permissionID)
    const dir = permission?.metadata?.directory as string | undefined
    if (dir && normalizeDir(dir) !== normalizeDir(state.directory)) {
      await replyElsewhere(sessionID, permissionID, response, dir)
    } else {
      const result = await requireClient().postSessionIdPermissionsPermissionId({
        path: { id: sessionID, permissionID },
        body: { response },
      })
      if (result.error) return
    }
    answered.add(permissionID)
    set(
      produce((s) => {
        s.permissions[sessionID] = (s.permissions[sessionID] ?? []).filter((p) => p.id !== permissionID)
      }),
    )
  }

  async function answerQuestion(sessionID: string, requestID: string, answers: string[][] | null) {
    const base = target()
    if (!base) return
    const question = (state.questions[sessionID] ?? []).find((item) => item.id === requestID)
    const dir = question?.directory ?? state.directory
    const action = answers ? "reply" : "reject"
    const url = `${base.url}/question/${requestID}/${action}?directory=${encodeURIComponent(dir)}`
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify(answers ? { answers } : {}),
    }).catch(() => null)
    if (!response?.ok) return
    answered.add(requestID)
    set(
      produce((s) => {
        s.questions[sessionID] = (s.questions[sessionID] ?? []).filter((item) => item.id !== requestID)
      }),
    )
  }

  async function replyElsewhere(sessionID: string, permissionID: string, response: PermissionResponse, dir: string) {
    const base = target()
    if (!base) return
    const url = `${base.url}/session/${sessionID}/permissions/${permissionID}?directory=${encodeURIComponent(dir)}`
    await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...base.headers },
      body: JSON.stringify({ response }),
    }).catch(() => {})
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

  return {
    openSession,
    loadOlder,
    loadSessions,
    removeAllSessions,
    newSession,
    fork,
    send,
    abort,
    summarize,
    share,
    unshare,
    findFiles,
    refreshProviders,
    providerAuthMethods,
    providerAuthorize,
    providerCallback,
    setProviderKey,
    mcpStatus,
    mcpConnect,
    mcpDisconnect,
    mcpAuthenticate,
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
