import type { OpencodeClient, Session } from "@opencode-ai/sdk/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { sessionBusy, spawnLink, type EngineState, type ModelRef } from "./store"

export type PromptOptions = { model: ModelRef | null; agent: string; variant?: string }
export type PermissionResponse = "once" | "always" | "reject"

export function createActions(
  requireClient: () => OpencodeClient,
  state: EngineState,
  set: SetStoreFunction<EngineState>,
) {
  async function reloadSession(id: string) {
    const result = await requireClient().session.messages({ path: { id } })
    set("transcripts", id, result.data ?? [])
    set("loaded", id, true)
    for (const entry of result.data ?? []) {
      for (const part of entry.parts) {
        const link = spawnLink(part)
        if (link) set("links", link.child, link.parent)
      }
    }
  }

  async function openSession(id: string) {
    if (state.loaded[id]) return
    await reloadSession(id)
  }

  async function loadSessions(directory: string) {
    const result = await requireClient().session.list({ query: { directory } })
    for (const session of result.data ?? []) set("sessions", session.id, session)
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
    if (result.data) set("sessions", id, result.data)
    return result.data?.share?.url
  }

  async function unshare(id: string) {
    const result = await requireClient().session.unshare({ path: { id } })
    if (result.data) set("sessions", id, { ...result.data, share: undefined })
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

  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    await requireClient().postSessionIdPermissionsPermissionId({
      path: { id: sessionID, permissionID },
      body: { response },
    })
  }

  async function revert(id: string, messageID: string) {
    if (sessionBusy(state, id)) await requireClient().session.abort({ path: { id } }).catch(() => {})
    const result = await requireClient().session.revert({ path: { id }, body: { messageID } })
    if (result.data) set("sessions", id, result.data)
    await reloadSession(id)
  }

  async function unrevert(id: string) {
    if (sessionBusy(state, id)) await requireClient().session.abort({ path: { id } }).catch(() => {})
    const result = await requireClient().session.unrevert({ path: { id } })
    if (result.data) set("sessions", id, result.data)
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
    replyPermission,
    revert,
    unrevert,
  }
}

export type EngineActions = ReturnType<typeof createActions>
