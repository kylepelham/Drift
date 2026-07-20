import type { OpencodeClient, Permission, Session } from "@opencode-ai/sdk/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { sleep, type EngineTarget } from "./connection"
import { normalizeDir, putSession, recordLink, sessionBusy, spawnLink, type EngineState, type ModelRef } from "./store"

export type PromptOptions = { model: ModelRef | null; agent: string; variant?: string }
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
  async function reloadSession(id: string) {
    const result = await requireClient().session.messages({ path: { id } })
    set("transcripts", id, result.data ?? [])
    set("loaded", id, true)
    for (const entry of result.data ?? []) {
      for (const part of entry.parts) {
        const link = spawnLink(part)
        if (!link) continue
        recordLink(link)
        set("links", link.child, link.parent)
      }
    }
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
      parts: [{ type: "text" as const, text }],
      model: options.model ?? undefined,
      agent: options.agent,
      ...(options.variant ? { variant: options.variant } : {}),
    }
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

  // The generated SDK lags the engine here; GET /permission recovers asks raised while
  // we weren't listening, across every workspace directory.
  async function refreshPermissions(directories: string[]) {
    const base = target()
    if (!base) return
    const results = await Promise.all(
      directories.map(async (dir) => {
        const url = `${base.url}/permission?directory=${encodeURIComponent(dir)}`
        const response = await fetch(url, { headers: base.headers }).catch(() => null)
        if (!response?.ok) return []
        const requests = ((await response.json().catch(() => [])) ?? []) as PermissionRequest[]
        return requests.map((request) => toPermission(request, dir))
      }),
    )
    const reported = new Set(results.flat().map((permission) => permission.id))
    for (const id of answered) if (!reported.has(id)) answered.delete(id)
    set(
      produce((s) => {
        s.permissions = {}
        for (const permission of results.flat())
          if (!answered.has(permission.id)) (s.permissions[permission.sessionID] ??= []).push(permission)
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
    if (!sessionBusy(state, id)) return
    await requireClient().session.abort({ path: { id } }).catch(() => {})
    for (let waited = 0; waited < 5000 && sessionBusy(state, id); waited += 100) await sleep(100)
  }

  async function revert(id: string, messageID: string) {
    await interrupt(id)
    const result = await requireClient().session.revert({ path: { id }, body: { messageID } })
    if (result.error) {
      set("errors", id, "Revert failed: the engine rejected the request")
      return
    }
    set("errors", id, undefined!)
    if (result.data) putSession(set, result.data)
    await reloadSession(id)
  }

  async function unrevert(id: string) {
    await interrupt(id)
    const result = await requireClient().session.unrevert({ path: { id } })
    if (result.data) putSession(set, result.data)
    await reloadSession(id)
  }

  return {
    openSession,
    loadSessions,
    removeAllSessions,
    newSession,
    fork,
    send,
    abort,
    summarize,
    share,
    unshare,
    runCommand,
    rename,
    remove,
    refreshPermissions,
    replyPermission,
    revert,
    unrevert,
  }
}

export type EngineActions = ReturnType<typeof createActions>
