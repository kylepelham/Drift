export type Workspace = { id: string; path: string; name: string; icon: string; lastUsed: number; removedAt?: number }
export type ArchivedSession = { sessionId: string; workspaceId: string; archivedAt: number }
export type PendingSessionDeletion = { sessionId: string; directory: string; claim: string }
export type DeletionSweep = { pending: PendingSessionDeletion[]; workspaces: Workspace[] }
export type McpConfig = Record<string, unknown> & { type: "local" | "remote" }
export type StoredMcpServer = { name: string; config: McpConfig; updatedAt: number }
export type McpDecision = "pending" | "approved" | "rejected" | "invalid"
export type ObservedMcpServer = {
  name: string
  type: "local" | "remote"
  fingerprint: string
  decision: McpDecision
}
export type McpSnapshot = {
  generation: number
  directory: string
  servers: StoredMcpServer[]
  observed: ObservedMcpServer[]
}

export interface DriftStore {
  workspaces(): Promise<Workspace[]>
  removedWorkspaces(): Promise<Workspace[]>
  addWorkspace(workspace: Omit<Workspace, "lastUsed">): Promise<Workspace>
  saveWorkspace(workspace: Omit<Workspace, "lastUsed">): Promise<void>
  touchWorkspace(id: string): Promise<void>
  removeWorkspace(id: string): Promise<void>
  prepareDeletions(before: number): Promise<DeletionSweep>
  stageWorkspaceDeletion(workspaceId: string, sessionIds: string[]): Promise<PendingSessionDeletion[]>
  claimDeletions(entries: PendingSessionDeletion[]): Promise<PendingSessionDeletion[]>
  confirmDeletions(entries: PendingSessionDeletion[]): Promise<void>
  archived(): Promise<ArchivedSession[]>
  archiveSession(sessionId: string, workspaceId: string): Promise<void>
  unarchiveSession(sessionId: string): Promise<void>
  mcpSnapshot(directory: string): Promise<McpSnapshot>
  saveMcp(name: string, config: McpConfig, generation: number, previousName?: string): Promise<void>
  removeMcp(name: string, generation: number): Promise<void>
  approveMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
  rejectMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
  revokeMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
}

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

export function shellInvoke(): Invoke | undefined {
  return (globalThis as { __TAURI__?: { core?: { invoke: Invoke } } }).__TAURI__?.core?.invoke
}

function shellStore(invoke: Invoke): DriftStore {
  return {
    workspaces: () => invoke("store_workspaces"),
    removedWorkspaces: () => invoke("store_removed_workspaces"),
    addWorkspace: (w) => invoke("store_add_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    saveWorkspace: (w) => invoke("store_save_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    touchWorkspace: (id) => invoke("store_touch_workspace", { id }),
    removeWorkspace: (id) => invoke("store_remove_workspace", { id }),
    prepareDeletions: (before) => invoke("store_prepare_deletions", { before }),
    stageWorkspaceDeletion: (workspaceId, sessionIds) =>
      invoke("store_stage_workspace_deletion", { workspaceId, sessionIds }),
    claimDeletions: (entries) => invoke("store_claim_deletions", { entries }),
    confirmDeletions: (entries) => invoke("store_confirm_deletions", { entries }),
    archived: () => invoke("store_archived"),
    archiveSession: (sessionId, workspaceId) => invoke("store_archive_session", { sessionId, workspaceId }),
    unarchiveSession: (sessionId) => invoke("store_unarchive_session", { sessionId }),
    mcpSnapshot: (directory) => invoke("mcp_snapshot", { directory }),
    saveMcp: (name, config, generation, previousName) =>
      invoke("mcp_save", { name, config, generation, previousName }),
    removeMcp: (name, generation) => invoke("mcp_remove", { name, generation }),
    approveMcp: (directory, name, fingerprint, generation) =>
      invoke("mcp_approve", { directory, name, fingerprint, generation }),
    rejectMcp: (directory, name, fingerprint, generation) =>
      invoke("mcp_reject", { directory, name, fingerprint, generation }),
    revokeMcp: (directory, name, fingerprint, generation) =>
      invoke("mcp_revoke", { directory, name, fingerprint, generation }),
  }
}

function read<T>(key: string, initial: T): T {
  const raw = localStorage.getItem(key)
  return raw ? (JSON.parse(raw) as T) : initial
}

function write(key: string, value: unknown) {
  localStorage.setItem(key, JSON.stringify(value))
}

type StoredWorkspace = Workspace & { removedAt?: number; purgeStagedAt?: number }

function browserStore(): DriftStore {
  const wsKey = "drift.store.workspaces"
  const arKey = "drift.store.archived"
  const pendingKey = "drift.store.pending-session-delete"
  const all = () => read<StoredWorkspace[]>(wsKey, [])
  const desktopMcpOnly = async (): Promise<never> => {
    throw new Error("MCP policy requires the Drift desktop backend")
  }
  return {
    workspaces: async () => all().filter((w) => !w.removedAt).sort((a, b) => b.lastUsed - a.lastUsed),
    removedWorkspaces: async () => all().filter((w) => w.removedAt).sort((a, b) => (b.removedAt ?? 0) - (a.removedAt ?? 0)),
    addWorkspace: async (w) => {
      const existing = all().find((x) => x.path === w.path)
      if (existing) {
        const restored = { ...existing, removedAt: undefined, purgeStagedAt: undefined, lastUsed: Date.now() }
        write(wsKey, [...all().filter((x) => x.id !== existing.id), restored])
        write(
          pendingKey,
          read<(PendingSessionDeletion & { workspaceId?: string })[]>(pendingKey, []).filter(
            (entry) => entry.workspaceId !== existing.id,
          ),
        )
        return restored
      }
      const created = { ...w, lastUsed: Date.now() }
      write(wsKey, [...all(), created])
      return created
    },
    saveWorkspace: async (w) => {
      write(wsKey, all().map((x) => (x.id === w.id ? { ...x, path: w.path, name: w.name, icon: w.icon } : x)))
    },
    touchWorkspace: async (id) => {
      write(wsKey, all().map((w) => (w.id === id ? { ...w, lastUsed: Date.now() } : w)))
    },
    removeWorkspace: async (id) => {
      write(wsKey, all().map((w) => (w.id === id ? { ...w, removedAt: Date.now() } : w)))
    },
    prepareDeletions: async (before) => {
      const pending = read<(PendingSessionDeletion & { workspaceId?: string })[]>(pendingKey, [])
      for (const entry of pending) entry.claim ||= crypto.randomUUID()
      const archives = read<ArchivedSession[]>(arKey, []).filter((entry) => entry.archivedAt < before)
      for (const archive of archives) {
        if (pending.some((entry) => entry.sessionId === archive.sessionId)) continue
        const workspace = all().find((entry) => entry.id === archive.workspaceId)
        if (workspace)
          pending.push({
            sessionId: archive.sessionId,
            directory: workspace.path,
            workspaceId: archive.workspaceId,
            claim: crypto.randomUUID(),
          })
      }
      write(pendingKey, pending)
      return {
        pending: pending.map(({ sessionId, directory, claim }) => ({ sessionId, directory, claim })),
        workspaces: all().filter((workspace) => workspace.removedAt && workspace.removedAt < before && !workspace.purgeStagedAt),
      }
    },
    stageWorkspaceDeletion: async (workspaceId, sessionIds) => {
      const workspace = all().find((entry) => entry.id === workspaceId && entry.removedAt)
      if (!workspace) return read<PendingSessionDeletion[]>(pendingKey, [])
      const pending = read<(PendingSessionDeletion & { workspaceId?: string })[]>(pendingKey, [])
      const archived = read<ArchivedSession[]>(arKey, [])
        .filter((entry) => entry.workspaceId === workspaceId)
        .map((entry) => entry.sessionId)
      for (const sessionId of [...sessionIds, ...archived]) {
        const existing = pending.find((entry) => entry.sessionId === sessionId)
        if (existing) {
          existing.directory = workspace.path
          existing.workspaceId = workspaceId
        } else {
          pending.push({ sessionId, directory: workspace.path, workspaceId, claim: crypto.randomUUID() })
        }
      }
      write(pendingKey, pending)
      write(wsKey, all().map((entry) => (entry.id === workspaceId ? { ...entry, purgeStagedAt: Date.now() } : entry)))
      return pending.map(({ sessionId, directory, claim }) => ({ sessionId, directory, claim }))
    },
    claimDeletions: async (entries) => {
      const pending = read<PendingSessionDeletion[]>(pendingKey, [])
      return entries.filter((entry) =>
        pending.some((item) => item.sessionId === entry.sessionId && item.claim === entry.claim),
      )
    },
    confirmDeletions: async (entries) => {
      const confirmed = new Map(entries.map((entry) => [entry.sessionId, entry.claim]))
      const pending = read<(PendingSessionDeletion & { workspaceId?: string })[]>(pendingKey, []).filter(
        (entry) => confirmed.get(entry.sessionId) !== entry.claim,
      )
      write(pendingKey, pending)
      const removed = new Set(entries.filter((entry) => !pending.some((item) => item.sessionId === entry.sessionId)).map((entry) => entry.sessionId))
      write(arKey, read<ArchivedSession[]>(arKey, []).filter((entry) => !removed.has(entry.sessionId)))
      write(
        wsKey,
        all().filter(
          (workspace) =>
            !workspace.removedAt ||
            !workspace.purgeStagedAt ||
            pending.some((entry) => entry.workspaceId === workspace.id),
        ),
      )
    },
    archived: async () => read<ArchivedSession[]>(arKey, []),
    archiveSession: async (sessionId, workspaceId) => {
      const list = read<ArchivedSession[]>(arKey, []).filter((a) => a.sessionId !== sessionId)
      write(arKey, [...list, { sessionId, workspaceId, archivedAt: Date.now() }])
    },
    unarchiveSession: async (sessionId) => {
      write(arKey, read<ArchivedSession[]>(arKey, []).filter((a) => a.sessionId !== sessionId))
      write(
        pendingKey,
        read<PendingSessionDeletion[]>(pendingKey, []).filter((entry) => entry.sessionId !== sessionId),
      )
    },
    mcpSnapshot: desktopMcpOnly,
    saveMcp: desktopMcpOnly,
    removeMcp: desktopMcpOnly,
    approveMcp: desktopMcpOnly,
    rejectMcp: desktopMcpOnly,
    revokeMcp: desktopMcpOnly,
  }
}

const invoke = shellInvoke()
export const driftStore: DriftStore = invoke ? shellStore(invoke) : browserStore()
