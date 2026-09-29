// Everything the UI asks the engine to do. Runs against the native engine; legacy shapes via adapt.
import type { Permission, Session } from "@opencode-ai/sdk/client"
import { produce, type SetStoreFunction } from "solid-js/store"
import { t } from "../state/i18n"
import { applyProviderCatalog } from "../state/provider-cache"
import { applySessionSnapshot, applyStatusSnapshot, pushNotice } from "./events"
import { adaptMessage, adaptPart, adaptPermission, adaptProvider, adaptQuestion, adaptSession, adaptTodos, type NativeMessageWithParts, type WorkspaceIndex } from "./native/adapt"
import { EngineError, type Client } from "./native/client"
import {
  captureRevisions,
  compareMessages,
  interruptStaleTools,
  mergeTranscriptSnapshot,
  putSession,
  type EngineState,
  type MessageEntry,
  type ModelRef,
  type Notice,
} from "./store"

export type PromptFile = {
  filename?: string
  mime: string
  url: string
  source?: { type: "file"; path: string; text: { value: string; start: number; end: number } }
}
export type PromptOptions = { model: ModelRef | null; agent: string; variant?: string; files?: PromptFile[]; directory?: string }
export type PromptSendResult = { ok: true } | { ok: false; error: string }
export type PermissionResponse = "once" | "always" | "reject"
export type ProviderAuthResult = { ok: boolean; connected: boolean }
export type SessionMoveResult = { ok: boolean; moved: string[]; error?: string }

const pageSize = 100
const sessionPageSize = 200
/** Reasoning effort names the composer offers, as thinking budgets in tokens. */
const thinkingBudgets: Record<string, number> = { low: 4_000, medium: 10_000, high: 20_000, max: 32_000 }
const oauthProviders = ["anthropic"]

export function createActions(
  requireClient: () => Client,
  state: EngineState,
  set: SetStoreFunction<EngineState>,
  workspaces: () => WorkspaceIndex,
) {
  const transcriptRequests = new Map<string, Promise<boolean>>()
  let noticeSequence = 0

  function notice(input: Omit<Notice, "id" | "created" | "duration"> & { id?: string; created?: number; duration?: number }) {
    pushNotice(set, {
      id: input.id ?? `notice-${Date.now()}-${noticeSequence++}`,
      created: input.created ?? Date.now(),
      duration: input.duration ?? 5000,
      ...input,
    })
  }

  function unavailable(feature: string) {
    notice({ id: `unavailable-${feature}`, title: "Not available yet", message: `${feature} is not part of the native engine yet.`, variant: "info" })
  }

  function entries(messages: NativeMessageWithParts[], directory: string): MessageEntry[] {
    return messages.map(({ parts, ...info }) => ({ info: adaptMessage(info, directory), parts: parts.map(adaptPart) }))
  }

  async function reloadSession(id: string) {
    const captured = captureRevisions(state)
    const existed = id in state.sessions
    const messages = await requireClient().messages(id, { limit: pageSize })
    const directory = state.sessions[id]?.directory ?? ""
    const loaded = interruptStaleTools(entries(messages, directory).sort(compareMessages), state.liveTools, t("drift.message.interrupted"))
    if (existed && !state.sessions[id]) return
    set("transcripts", id, mergeTranscriptSnapshot(state.transcripts[id], loaded, id, captured, state.revisions))
    set("loaded", id, true)
    set("cursors", id, messages.length === pageSize ? messages[0]!.id : null)
    const todos = await requireClient().todos(id).catch(() => undefined)
    if (todos) set("todos", id, adaptTodos(todos))
  }

  function openSession(id: string) {
    if (state.loaded[id]) return Promise.resolve(true)
    const active = transcriptRequests.get(id)
    if (active) return active
    let request!: Promise<boolean>
    request = reloadSession(id)
      .then(() => true)
      .catch((cause) => {
        notice({ id: `transcript-load-${id}`, title: "Transcript load failed", message: errorMessage(cause), variant: "error" })
        return false
      })
      .finally(() => {
        if (transcriptRequests.get(id) === request) transcriptRequests.delete(id)
      })
    transcriptRequests.set(id, request)
    return request
  }

  async function loadOlder(id: string) {
    const cursor = state.cursors[id]
    if (!cursor) return false
    const older = await requireClient().messages(id, { before: cursor, limit: pageSize })
    const directory = state.sessions[id]?.directory ?? ""
    const sorted = interruptStaleTools(entries(older, directory).sort(compareMessages), state.liveTools, t("drift.message.interrupted"))
    set(
      produce((draft) => {
        const existing = new Set((draft.transcripts[id] ?? []).map((entry) => entry.info.id))
        draft.transcripts[id] = [...sorted.filter((entry) => !existing.has(entry.info.id)), ...(draft.transcripts[id] ?? [])]
      }),
    )
    set("cursors", id, older.length === pageSize ? older[0]!.id : null)
    return sorted.length > 0
  }

  /** Every page of a listing; a truncated snapshot would purge sessions it never saw. */
  async function allPages(params: { workspace?: string; archived?: boolean }) {
    const all: Session[] = []
    let before: string | undefined
    for (;;) {
      const page = await requireClient().sessions({ ...params, before, limit: sessionPageSize })
      all.push(...page.map((s) => adaptSession(s, workspaces())))
      if (page.length < sessionPageSize) return { sessions: all, running: page.filter((s) => s.running).map((s) => s.id) }
      before = page[page.length - 1]!.id
    }
  }

  /** The engine says which sessions have a turn in flight; everything else listed is idle. */
  function reconcileStatus(sessions: Session[], running: Set<string>, captured: Record<string, number>) {
    const statuses = Object.fromEntries(sessions.map((s) => [s.id, running.has(s.id) ? { type: "busy" as const } : { type: "idle" as const }]))
    applyStatusSnapshot(set, { sessions, statuses, captured })
  }

  async function loadSessions(directory: string) {
    const workspace = workspaces().id(directory)
    if (!workspace) return
    const captured = captureRevisions(state)
    const epoch = state.sessionSnapshotEpoch
    const { sessions, running } = await allPages({ workspace })
    if (state.sessionSnapshotEpoch !== epoch) return
    applySessionSnapshot(set, { sessions, captured, scope: { directory } })
    reconcileStatus(sessions, new Set(running), captured)
  }

  async function loadAllSessions() {
    const captured = captureRevisions(state)
    const [live, archived] = await Promise.all([allPages({}), allPages({ archived: true })])
    const sessions = [...live.sessions, ...archived.sessions]
    applySessionSnapshot(set, { sessions, captured })
    reconcileStatus(sessions, new Set(live.running), captured)
    set("sessionSnapshotAll", true)
  }

  async function newSession(): Promise<(Session & { discard: () => Promise<void> }) | undefined> {
    const workspaceId = workspaces().id(state.directory)
    if (!workspaceId) return undefined
    const created = await requireClient().createSession({ workspaceId })
    const session = adaptSession(created, workspaces())
    // A fresh session is known empty; mark it loaded so the first turn's events are not dropped.
    set(
      produce((draft) => {
        draft.transcripts[session.id] ??= []
        draft.loaded[session.id] = true
        draft.cursors[session.id] ??= null
      }),
    )
    putSession(set, session)
    return { ...session, discard: () => purgeSession(session.id).then(() => undefined) }
  }

  async function send(id: string, text: string, options: PromptOptions): Promise<PromptSendResult> {
    set("errors", id, undefined!)
    const parts = [
      ...(text.trim() ? [{ type: "text" as const, text }] : []),
      ...(options.files ?? []).map((file) => ({ type: "file" as const, mime: file.mime, name: file.filename ?? "file", url: file.url })),
    ]
    if (parts.length === 0) return fail(id, "Prompt failed: the prompt is empty")
    try {
      await requireClient().submit(id, {
        submissionId: submissionId(),
        parts,
        model: options.model ? { provider: options.model.providerID, model: options.model.modelID } : undefined,
        thinkingBudget: options.variant ? thinkingBudgets[options.variant] : undefined,
      })
      return { ok: true }
    } catch (cause) {
      return fail(id, `Prompt failed: ${errorMessage(cause)}`)
    }
  }

  function fail(id: string, error: string): PromptSendResult {
    set("errors", id, error)
    return { ok: false, error }
  }

  async function abort(id: string) {
    await requireClient().abort(id)
  }

  async function rename(id: string, title: string) {
    const updated = await requireClient().updateSession(id, { title })
    putSession(set, adaptSession(updated, workspaces()))
  }

  /** Archives; the engine never deletes a session outright. */
  async function remove(id: string) {
    await requireClient().updateSession(id, { archived: true })
    set(produce((draft) => purge(draft, id)))
  }

  /// True only when the engine confirmed the archive; the caller decides what a failure means.
  async function purgeSession(id: string) {
    try {
      await remove(id)
      return true
    } catch (cause) {
      if (cause instanceof EngineError && cause.status === 404) return true
      return false
    }
  }

  async function refreshProviders() {
    const providers = await requireClient().providers().catch(() => undefined)
    if (!providers) return false
    applyProviderCatalog(set, {
      all: providers.map(adaptProvider),
      connected: providers.filter((p) => p.connected).map((p) => p.id),
      default: {},
    })
    return true
  }

  /// Pending asks of both kinds; a single fetch each, applied together so the UI never sees a gap.
  async function refreshPermissions(_directories: string[] = []) {
    const [permissions, questions] = await Promise.all([requireClient().permissions(), requireClient().questions()])
    set(
      produce((draft) => {
        draft.permissions = {}
        draft.questions = {}
        for (const request of permissions) {
          const directory = draft.sessions[request.sessionId]?.directory ?? ""
          const permission: Permission = adaptPermission(request, directory)
          ;(draft.permissions[request.sessionId] ??= []).push(permission)
        }
        for (const request of questions) (draft.questions[request.sessionId] ??= []).push(adaptQuestion(request))
      }),
    )
  }

  async function answerQuestion(sessionID: string, requestID: string, answers: string[][] | null) {
    try {
      if (answers) await requireClient().answerQuestion(requestID, answers)
      else await requireClient().rejectQuestion(requestID)
    } catch (cause) {
      if (!(cause instanceof EngineError && cause.status === 404)) throw cause
    }
    set(produce((draft) => void (draft.questions[sessionID] = (draft.questions[sessionID] ?? []).filter((q) => q.id !== requestID))))
  }

  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse) {
    const reply = response === "reject" ? "deny" : response
    try {
      await requireClient().replyPermission(permissionID, { reply })
    } catch (cause) {
      if (cause instanceof EngineError && cause.status === 404) {
        set(produce((draft) => void (draft.permissions[sessionID] = (draft.permissions[sessionID] ?? []).filter((p) => p.id !== permissionID))))
        return
      }
      throw cause
    }
  }

  async function setProviderKey(id: string, key: string): Promise<ProviderAuthResult> {
    await requireClient().setProviderKey(id, key)
    await refreshProviders()
    return { ok: true, connected: state.connected.includes(id) }
  }

  async function disconnectProvider(id: string): Promise<ProviderAuthResult> {
    await requireClient().removeProviderCredentials(id)
    await refreshProviders()
    return { ok: true, connected: state.connected.includes(id) }
  }

  async function providerAuthMethods(): Promise<Record<string, { type: "oauth" | "api"; label: string }[]>> {
    return Object.fromEntries(oauthProviders.map((id) => [id, [{ type: "oauth", label: "Claude Pro/Max" }, { type: "api", label: "API key" }]]))
  }

  async function providerAuthorize(id: string, _method: number) {
    const started = await requireClient().startOAuth(id, "max")
    return { url: started.url, method: "code" as "code" | "auto", instructions: "Sign in, then paste the code the page shows." }
  }

  async function providerCallback(id: string, _method: number, code?: string): Promise<ProviderAuthResult> {
    if (!code) return { ok: false, connected: false }
    await requireClient().finishOAuth(id, code)
    await refreshProviders()
    return { ok: true, connected: state.connected.includes(id) }
  }

  const notYet = (feature: string) => async (..._args: unknown[]) => {
    unavailable(feature)
    return undefined
  }
  const never = (feature: string) => async (..._args: unknown[]) => {
    unavailable(feature)
    return false
  }

  return {
    openSession,
    loadOlder,
    loadSessions,
    loadAllSessions,
    newSession,
    send,
    abort,
    rename,
    remove,
    purgeSession,
    refreshProviders,
    reloadProviders: refreshProviders,
    refreshPermissions,
    replyPermission,
    answerQuestion,
    setProviderKey,
    disconnectProvider,
    providerAuthMethods,
    providerAuthorize,
    providerCallback,
    notice,
    refreshAgents: async () => undefined,
    findFiles: async (_query: string): Promise<string[]> => [],
    steer: async (id: string, text: string, options: PromptOptions) => send(id, text, options),
    fork: async (..._args: unknown[]): Promise<Session | undefined> => {
      unavailable("Forking")
      return undefined
    },
    spawn: notYet("Spawned threads"),
    moveSession: async (..._args: unknown[]): Promise<SessionMoveResult> => ({ ok: false, moved: [], error: "Moving sessions is not available yet" }),
    moveWorkspaceSessions: async (..._args: unknown[]): Promise<SessionMoveResult> => ({ ok: false, moved: [], error: "Moving sessions is not available yet" }),
    removeAllSessions: async (..._args: unknown[]) => false,
    switchRetryModel: async (..._args: unknown[]): Promise<PromptSendResult> => ({ ok: false, error: "Retry model switching is not available yet" }),
    summarize: notYet("Compaction"),
    share: async (..._args: unknown[]): Promise<string | undefined> => {
      unavailable("Sharing")
      return undefined
    },
    unshare: notYet("Sharing"),
    runCommand: notYet("Commands"),
    revert: never("Revert"),
    unrevert: never("Revert"),
    mcpInitialize: async (_directory: string) => undefined,
    mcpStatus: async (_directory: string, _signal: AbortSignal) => ({}),
    mcpConnect: async (_name: string, _directory: string) => unavailable("MCP"),
    mcpDisconnect: async (_name: string, _directory: string) => unavailable("MCP"),
    mcpAuthenticate: async (_name: string, _directory: string) => unavailable("MCP"),
  }
}

export type EngineActions = ReturnType<typeof createActions>

/** A retried send with the same id gets the original receipt instead of a second turn. */
function submissionId() {
  return typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `sub_${Date.now()}_${Math.random().toString(36).slice(2)}`
}

function purge(draft: EngineState, id: string) {
  delete draft.sessions[id]
  delete draft.transcripts[id]
  delete draft.loaded[id]
  delete draft.permissions[id]
  delete draft.status[id]
  delete draft.errors[id]
  delete draft.cursors[id]
}

export function errorMessage(cause: unknown) {
  if (cause instanceof EngineError) return cause.message
  if (cause instanceof Error) return cause.message
  return String(cause)
}
