// Everything the UI asks the engine to do. Runs against the native engine; legacy shapes via adapt.
import type { Command, Permission, Session } from "@opencode-ai/sdk/client"
import { untrack } from "solid-js"
import { produce, reconcile, type SetStoreFunction } from "solid-js/store"
import { t } from "../state/i18n"
import { applyProviderCatalog } from "../state/provider-cache"
import { applySessionSnapshot, applyStatusSnapshot, pushNotice } from "./events"
import { adaptMessage, adaptPart, adaptPermission, adaptProvider, adaptQuestion, adaptSession, adaptTodos, type NativeMessageWithParts, type WorkspaceIndex } from "./native/adapt"
import { EngineError, type Client } from "./native/client"
import type { components } from "./native/types"
import {
  captureRevisions,
  compareMessages,
  interruptStaleTools,
  mergeTranscriptSnapshot,
  putSession,
  putTasks,
  savedChoice,
  type AgentInfo,
  type EngineState,
  type McpServerConfig,
  type McpServerStatus,
  type MessageEntry,
  type ModelRef,
  type Notice,
} from "./store"

export type BranchDraft = components["schemas"]["BranchDraft"]
type NativeSession = components["schemas"]["Session"]

export type PromptFile = {
  filename?: string
  mime: string
  url: string
  source?: { type: "file"; path: string; text: { value: string; start: number; end: number } }
}
/** `variant` null asks for the model's default level; left out, the session keeps its own (as for a level the model does not offer). */
export type PromptOptions = { model: ModelRef | null; agent: string; variant?: string | null; files?: PromptFile[]; directory?: string }
/** A prompt the engine gave back unrun: discarded, stopped, or replaced by a newer one while it waited. */
export type ReturnedPrompt = { text: string; files: { mime: string; name: string; url: string }[] }
export type PromptSendResult = { ok: true; returned?: ReturnedPrompt } | { ok: false; error: string }
/** What became of an archived thread due for purging: gone, restored and kept, or not reached this time. */
export type ArchivePurge = "deleted" | "kept" | "failed"
export type PermissionResponse = "once" | "always" | "reject" | "stop"
export type ProviderAuthResult = { ok: boolean; connected: boolean }
export type SessionMoveResult = { ok: boolean; moved: string[]; error?: string }

const pageSize = 100
const sessionPageSize = 200
/** Sign-in methods per provider, in the order the settings page lists them. */
const authMethods: Record<string, { type: "oauth" | "api"; label: string; mode?: "max" | "console" | "chatgpt" }[]> = {
  anthropic: [
    { type: "oauth", label: "Claude Pro/Max", mode: "max" },
    { type: "oauth", label: "Anthropic Console", mode: "console" },
    { type: "api", label: "API key" },
  ],
  openai: [
    { type: "oauth", label: "ChatGPT (Plus, Pro, Team)", mode: "chatgpt" },
    { type: "api", label: "API key" },
  ],
}

export function createActions(
  requireClient: () => Client,
  state: EngineState,
  set: SetStoreFunction<EngineState>,
  workspaces: () => WorkspaceIndex,
) {
  const transcriptRequests = new Map<string, Promise<boolean>>()
  /** Submission ids of prompts whose fate is unknown, by session and exact prompt, until the engine answers for sure. */
  const unsettled = new Map<string, string>()
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
    const [todos, tasks] = await Promise.all([requireClient().todos(id).catch(() => undefined), requireClient().tasks(id).catch(() => undefined)])
    if (todos) set("todos", id, adaptTodos(todos))
    if (tasks) putTasks(set, state, id, tasks)
  }

  /** Stops one worker; the engine's `task.updated` reports how it ended. */
  async function stopTask(taskId: string) {
    try {
      const task = await requireClient().stopTask(taskId)
      putTasks(set, state, task.parentSessionId, [task])
    } catch (cause) {
      notice({ id: `task-stop-${taskId}`, title: t("drift.task.stopFailed"), message: errorMessage(cause), variant: "error" })
    }
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
    const running: string[] = []
    let before: string | undefined
    for (;;) {
      const page = await requireClient().sessions({ ...params, before, limit: sessionPageSize })
      all.push(...page.map((s) => adaptSession(s, workspaces())))
      running.push(...page.filter((s) => s.running).map((s) => s.id))
      if (page.length < sessionPageSize) return { sessions: all, running }
      before = page[page.length - 1]!.id
    }
  }

  /** The engine says which sessions have a turn in flight; everything else listed is idle. */
  function reconcileStatus(sessions: Session[], running: Set<string>, captured: Record<string, number>) {
    const statuses = Object.fromEntries(sessions.map((s) => [s.id, running.has(s.id) ? { type: "busy" as const } : { type: "idle" as const }]))
    applyStatusSnapshot(set, { sessions, statuses, captured })
  }

  // Loaders snapshot state untracked: effects call them, and they write what they read.
  async function loadSessions(directory: string) {
    const { workspace, captured, epoch } = untrack(() => ({ workspace: workspaces().id(directory), captured: captureRevisions(state), epoch: state.sessionSnapshotEpoch }))
    if (!workspace) return
    const { sessions, running } = await allPages({ workspace })
    if (state.sessionSnapshotEpoch !== epoch) return
    applySessionSnapshot(set, { sessions, captured, scope: { directory } })
    reconcileStatus(sessions, new Set(running), captured)
  }

  async function loadAllSessions() {
    const captured = untrack(() => captureRevisions(state))
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
    // Named only when they change what the session runs as next; an unchanged follow-up steers into the running turn.
    const saved = savedChoice(state, id)
    const prompt = {
      parts,
      model: options.model ? { provider: options.model.providerID, model: options.model.modelID } : undefined,
      ...(options.variant !== undefined && options.variant !== (saved.variant ?? null) ? { variant: options.variant } : {}),
      ...(options.agent && options.agent !== saved.agent ? { agent: options.agent } : {}),
    }
    // Resending the same prompt reuses its id, so a send whose answer was lost is not admitted twice.
    const key = `${id}\n${JSON.stringify(prompt)}`
    const submission = unsettled.get(key) ?? submissionId()
    unsettled.set(key, submission)
    try {
      const receipt = await requireClient().submit(id, { submissionId: submission, ...prompt })
      unsettled.delete(key)
      putSession(set, adaptSession(receipt.session, workspaces()))
      const returned = returnedPrompt(receipt.returned)
      return returned ? { ok: true, returned } : { ok: true }
    } catch (cause) {
      if (definite(cause)) unsettled.delete(key)
      return fail(id, `Prompt failed: ${errorMessage(cause)}`)
    }
  }

  function fail(id: string, error: string): PromptSendResult {
    set("errors", id, error)
    return { ok: false, error }
  }

  /** Stops the turn; a prompt that was waiting for it never runs and comes back. */
  async function abort(id: string) {
    return returnedPrompt((await requireClient().abort(id)).returned)
  }

  /** Takes back the prompt waiting for the turn; the turn carries on. */
  async function discardQueued(id: string) {
    try {
      return returnedPrompt((await requireClient().discardQueued(id)).returned)
    } catch (cause) {
      if (!(cause instanceof EngineError && cause.status === 404)) notice({ id: `discard-${id}`, title: "Couldn't discard", message: errorMessage(cause), variant: "error" })
      return undefined
    }
  }

  async function rename(id: string, title: string) {
    const updated = await requireClient().updateSession(id, { title })
    putSession(set, adaptSession(updated, workspaces()))
  }

  /** Archiving stops whatever the session is running, in the engine, before anything else hides it. */
  async function setArchived(id: string, archived: boolean) {
    const updated = await requireClient().updateSession(id, { archived })
    putSession(set, adaptSession(updated, workspaces()))
  }

  /// Permanent deletion; true only once the engine confirms the row is gone.
  async function purgeSession(id: string) {
    try {
      await requireClient().deleteSession(id)
      set(produce((draft) => purge(draft, id)))
      return true
    } catch (cause) {
      if (cause instanceof EngineError && cause.status === 404) return true
      return false
    }
  }

  /** The archive purge: the engine deletes only what is still archived, so a thread restored meanwhile is `kept`. */
  async function purgeArchivedSession(id: string): Promise<ArchivePurge> {
    try {
      await requireClient().purgeArchivedSession(id)
      set(produce((draft) => purge(draft, id)))
      return "deleted"
    } catch (cause) {
      if (cause instanceof EngineError && cause.status === 404) return "deleted"
      if (cause instanceof EngineError && cause.status === 409) return "kept"
      return "failed"
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

  /** `message` goes to the model with a refusal; `stop` refuses and ends the turn. */
  async function replyPermission(sessionID: string, permissionID: string, response: PermissionResponse, message?: string) {
    const reply = response === "reject" ? "deny" : response
    const refusing = reply === "deny" || reply === "stop"
    try {
      await requireClient().replyPermission(permissionID, { reply, ...(refusing && message?.trim() ? { message: message.trim() } : {}) })
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
    return Object.fromEntries(Object.entries(authMethods).map(([id, methods]) => [id, methods.map(({ type, label }) => ({ type, label }))]))
  }

  // The state from startOAuth, needed by the callback for flows the engine completes itself.
  const oauthStates = new Map<string, string>()

  async function providerAuthorize(id: string, method: number) {
    const mode = authMethods[id]?.[method]?.mode
    if (!mode) throw new Error("this method has no sign-in flow")
    const started = await requireClient().startOAuth(id, mode)
    oauthStates.set(id, started.state)
    const auto = started.method === "auto"
    return { url: started.url, method: (auto ? "auto" : "code") as "code" | "auto", instructions: auto ? "Finish signing in in the browser." : "Sign in, then paste the code the page shows." }
  }

  async function providerCallback(id: string, _method: number, code?: string): Promise<ProviderAuthResult> {
    const oauthState = oauthStates.get(id)
    if (!code && !oauthState) return { ok: false, connected: false }
    try {
      await requireClient().finishOAuth(id, code ?? "", oauthState)
    } catch (cause) {
      notice({ id: `oauth-${id}`, title: "Sign-in failed", message: errorMessage(cause), variant: "error", duration: 10_000 })
      return { ok: false, connected: false }
    } finally {
      oauthStates.delete(id)
    }
    await refreshProviders()
    return { ok: true, connected: state.connected.includes(id) }
  }

  /** Copies finished history into a new conversation, through `atMessage` or else everything finished. The copy keeps compaction markers, so it sees the same context. */
  async function fork(id: string, atMessage?: string) {
    try {
      const session = adaptSession(await requireClient().forkSession(id, atMessage), workspaces())
      putSession(set, session)
      return session
    } catch (cause) {
      notice({ id: `fork-${id}`, title: "Couldn't fork", message: errorMessage(cause), variant: "error", duration: 10_000 })
    }
  }

  /** `/compact`: the engine summarises now with the compaction agent's model, so the composer's model does not apply. */
  async function summarize(id: string, _model?: unknown) {
    try {
      await requireClient().compactSession(id)
    } catch (cause) {
      notice({ id: `compact-${id}`, title: "Couldn't compact", message: errorMessage(cause), variant: "error", duration: 10_000 })
    }
  }

  /** Undoes back to a prompt, files included; the engine hides it and everything after it. */
  async function revert(id: string, messageID: string) {
    return applyUndo(id, () => requireClient().revertSession(id, messageID))
  }

  /** Redoes everything an undo hid, files included. */
  async function unrevert(id: string) {
    return applyUndo(id, () => requireClient().unrevertSession(id))
  }

  async function applyUndo(id: string, call: () => Promise<{ session: NativeSession; kept: string[]; unattributed: string[] }>) {
    try {
      const { session, kept, unattributed } = await call()
      putSession(set, adaptSession(session, workspaces()))
      // Files the user changed after the session did are never overwritten; say which.
      if (kept.length) notice({ id: `revert-kept-${id}`, title: "Kept your changes", message: `Left as you changed them: ${kept.join(", ")}`, variant: "info", duration: 10_000 })
      // A command's run shows what changed, not who changed it, so those files are never undone.
      if (unattributed.length) notice({ id: `revert-unattributed-${id}`, title: "Left files changed during commands", message: `Changed while a command ran, so not undone: ${unattributed.join(", ")}`, variant: "info", duration: 10_000 })
      return true
    } catch (cause) {
      notice({ id: `revert-${id}`, title: "Couldn't undo", message: errorMessage(cause), variant: "error", duration: 10_000 })
      return false
    }
  }

  /** Moves a turn that is waiting to retry onto `model` at `variant` (the model's default when unset); it retries at once. */
  async function switchRetryModel(id: string, _messageID: string, model: ModelRef, variant?: string): Promise<PromptSendResult> {
    try {
      await requireClient().switchRetryModel(id, { provider: model.providerID, model: model.modelID }, variant ?? null)
      return { ok: true }
    } catch (cause) {
      return { ok: false, error: errorMessage(cause) }
    }
  }

  async function engineSettings() {
    return requireClient().settings()
  }

  async function setAutoCompact(autoCompact: boolean) {
    return requireClient().putSettings({ autoCompact })
  }

  /** Moves a session with its subagents; the engine refuses while any of them is running. */
  async function moveSession(id: string, destination: string): Promise<SessionMoveResult> {
    const workspaceId = workspaces().id(destination)
    if (!workspaceId) return { ok: false, moved: [], error: "That workspace is not registered with the engine" }
    try {
      return { ok: true, moved: (await requireClient().moveSession(id, workspaceId)).moved }
    } catch (cause) {
      return { ok: false, moved: [], error: errorMessage(cause) }
    }
  }

  /** Sessions belong to the workspace, not its path, so re-pointing a folder moves nothing; a running turn must finish first. */
  async function moveWorkspaceSessions(from: string, _to: string): Promise<SessionMoveResult> {
    const workspace = workspaces().id(from)
    if (!workspace) return { ok: true, moved: [] }
    try {
      const { running } = await allPages({ workspace })
      if (running.length) return { ok: false, moved: [], error: "Stop the running threads in this workspace first; they keep the folder they started in." }
      return { ok: true, moved: [] }
    } catch (cause) {
      return { ok: false, moved: [], error: errorMessage(cause) }
    }
  }

  /** Asks the source's model for a handoff the user reviews; nothing is created yet. */
  async function draftBranch(id: string, goal: string): Promise<BranchDraft | undefined> {
    try {
      return await requireClient().draftBranch(id, goal)
    } catch (cause) {
      notice({ id: `branch-${id}`, title: "Couldn't draft the branch", message: errorMessage(cause), variant: "error", duration: 10_000 })
    }
  }

  /** Creates the reviewed branch; the engine starts it and it runs independently of its source. */
  async function branch(id: string, draft: BranchDraft) {
    try {
      const session = adaptSession(await requireClient().createBranch(id, draft), workspaces())
      set(
        produce((draft) => {
          draft.transcripts[session.id] ??= []
          draft.loaded[session.id] = true
          draft.cursors[session.id] ??= null
        }),
      )
      putSession(set, session)
      return session
    } catch (cause) {
      notice({ id: `branch-${id}`, title: "Couldn't create the branch", message: errorMessage(cause), variant: "error", duration: 10_000 })
    }
  }

  /** Paths in the current workspace for an @ mention; the engine ranks them and applies ignore rules. */
  async function findFiles(text: string): Promise<string[]> {
    const workspace = workspaces().id(state.directory)
    return workspace ? requireClient().findFiles(workspace, text) : []
  }

  /** Agents and commands come from the workspace's drift.json and .drift/ directory. */
  async function refreshAgents() {
    const workspace = workspaces().id(state.directory)
    if (!workspace) return
    const config = await requireClient().workspaceConfig(workspace)
    const agents: AgentInfo[] = config.agents.map((agent) => ({
      name: agent.name,
      description: agent.description,
      mode: agent.kind === "subagent" ? "subagent" : "primary",
      hidden: agent.kind === "action",
      builtIn: agent.builtin,
      tools: agent.tools ?? [],
      ...(agent.prompt ? { prompt: agent.prompt } : {}),
      ...(agent.steps ? { steps: agent.steps } : {}),
      ...(agent.model ? { model: { providerID: agent.model.provider, modelID: agent.model.model } } : {}),
    }))
    const commands: Command[] = config.commands.map((command) => ({ name: command.name, description: command.description, template: command.template }))
    set("agents", agents)
    set("commands", commands)
  }

  async function runCommand(id: string, command: string, args: string) {
    set("errors", id, undefined!)
    try {
      await requireClient().runCommand(id, command, args)
    } catch (cause) {
      fail(id, `Command failed: ${errorMessage(cause)}`)
    }
  }

  async function refreshMcp() {
    const servers = await requireClient().mcpServers()
    set("mcpServers", reconcile(Object.fromEntries(servers.map((server) => [server.name, server]))))
  }

  /** Runs one MCP change on the engine and records the state it reports back. */
  async function mcpChange(change: () => Promise<McpServerStatus>) {
    const server = await change()
    set("mcpServers", server.name, reconcile(server))
    return server
  }

  /** `create`: adding a server, which the engine refuses rather than replace one of the same name. */
  function mcpSave(name: string, config: McpServerConfig, options: { create?: boolean } = {}) {
    return mcpChange(() => requireClient().saveMcpServer(name, config, !!options.create))
  }

  /** The engine renames in one step and refuses a name already taken, so no other server is ever replaced. */
  async function mcpRename(from: string, to: string) {
    const renamed = await requireClient().renameMcpServer(from, to)
    set("mcpServers", produce((servers) => void delete servers[from]))
    set("mcpServers", renamed.name, reconcile(renamed))
    return renamed
  }

  async function mcpRemove(name: string) {
    await requireClient().removeMcpServer(name)
    set("mcpServers", produce((servers) => void delete servers[name]))
  }

  const notYet = (feature: string) => async (..._args: unknown[]) => {
    unavailable(feature)
    return undefined
  }

  return {
    openSession,
    loadOlder,
    loadSessions,
    loadAllSessions,
    newSession,
    send,
    abort,
    discardQueued,
    stopTask,
    rename,
    setArchived,
    purgeSession,
    purgeArchivedSession,
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
    refreshAgents,
    findFiles,
    steer: async (id: string, text: string, options: PromptOptions) => send(id, text, options),
    fork,
    draftBranch,
    branch,
    moveSession,
    moveWorkspaceSessions,
    // Pending native work (CHECKLIST): reports nothing deleted, so a removed workspace's purge never completes.
    removeAllSessions: async (..._args: unknown[]) => false,
    switchRetryModel,
    summarize,
    engineSettings,
    setAutoCompact,
    share: async (..._args: unknown[]): Promise<string | undefined> => {
      unavailable("Sharing")
      return undefined
    },
    unshare: notYet("Sharing"),
    runCommand,
    revert,
    unrevert,
    refreshMcp,
    mcpSave,
    mcpRename,
    mcpRemove,
    /** `hash` is the config the user reviewed; the engine refuses if it has changed since. */
    mcpApprove: (name: string, hash?: string) => mcpChange(() => requireClient().approveMcpServer(name, hash)),
    mcpSetEnabled: (name: string, enabled: boolean) => mcpChange(() => requireClient().setMcpServerEnabled(name, enabled)),
    mcpConnect: (name: string) => mcpChange(() => requireClient().connectMcpServer(name)),
    mcpDisconnect: (name: string) => mcpChange(() => requireClient().disconnectMcpServer(name)),
  }
}

export type EngineActions = ReturnType<typeof createActions>

/** The engine answered and refused: the prompt was not admitted, so a resend may be a new submission. */
function definite(cause: unknown) {
  return cause instanceof EngineError && cause.status >= 400 && cause.status < 500
}

/** The text and files of prompts the engine gave back, oldest first; `undefined` when it gave nothing back. */
export function returnedPrompt(parts: readonly components["schemas"]["Part"][] | undefined): ReturnedPrompt | undefined {
  if (!parts?.length) return undefined
  const texts: string[] = []
  const files: ReturnedPrompt["files"] = []
  for (const part of parts) {
    if (part.type === "text") texts.push(part.text)
    if (part.type === "file") files.push({ mime: part.mime, name: part.name, url: part.url })
  }
  return { text: texts.join("\n\n"), files }
}

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
  delete draft.tasks[id]
}

export function errorMessage(cause: unknown) {
  if (cause instanceof EngineError) return cause.message
  if (cause instanceof Error) return cause.message
  return String(cause)
}
