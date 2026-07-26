import { shellInvoke, type ShellInvoke } from "../shell"

export type Workspace = { id: string; path: string; name: string; icon: string; lastUsed: number; removedAt?: number }
export type ArchivedSession = { sessionId: string; workspaceId: string; archivedAt: number }
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
  purgeRemovedWorkspaces(before: number): Promise<string[]>
  archived(): Promise<ArchivedSession[]>
  archiveSession(sessionId: string, workspaceId: string): Promise<void>
  unarchiveSession(sessionId: string): Promise<void>
  purgeArchived(before: number): Promise<string[]>
  mcpSnapshot(directory: string): Promise<McpSnapshot>
  saveMcp(name: string, config: McpConfig, generation: number, previousName?: string): Promise<void>
  removeMcp(name: string, generation: number): Promise<void>
  approveMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
  rejectMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
  revokeMcp(directory: string, name: string, fingerprint: string, generation: number): Promise<void>
}

type Invoke = ShellInvoke

function shellStore(invoke: Invoke): DriftStore {
  return {
    workspaces: () => invoke("store_workspaces"),
    removedWorkspaces: () => invoke("store_removed_workspaces"),
    addWorkspace: (w) => invoke("store_add_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    saveWorkspace: (w) => invoke("store_save_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    touchWorkspace: (id) => invoke("store_touch_workspace", { id }),
    removeWorkspace: (id) => invoke("store_remove_workspace", { id }),
    purgeRemovedWorkspaces: (before) => invoke("store_purge_removed_workspaces", { before }),
    archived: () => invoke("store_archived"),
    archiveSession: (sessionId, workspaceId) => invoke("store_archive_session", { sessionId, workspaceId }),
    unarchiveSession: (sessionId) => invoke("store_unarchive_session", { sessionId }),
    purgeArchived: (before) => invoke("store_purge_archived", { before }),
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

type StoredWorkspace = Workspace & { removedAt?: number }

function browserStore(): DriftStore {
  const wsKey = "drift.store.workspaces"
  const arKey = "drift.store.archived"
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
        const restored = { ...existing, removedAt: undefined, lastUsed: Date.now() }
        write(wsKey, [...all().filter((x) => x.id !== existing.id), restored])
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
    purgeRemovedWorkspaces: async (before) => {
      const gone = all().filter((w) => w.removedAt && w.removedAt < before)
      write(wsKey, all().filter((w) => !gone.some((g) => g.id === w.id)))
      write(arKey, read<ArchivedSession[]>(arKey, []).filter((a) => !gone.some((g) => g.id === a.workspaceId)))
      return gone.map((w) => w.path)
    },
    archived: async () => read<ArchivedSession[]>(arKey, []),
    archiveSession: async (sessionId, workspaceId) => {
      const list = read<ArchivedSession[]>(arKey, []).filter((a) => a.sessionId !== sessionId)
      write(arKey, [...list, { sessionId, workspaceId, archivedAt: Date.now() }])
    },
    unarchiveSession: async (sessionId) => {
      write(arKey, read<ArchivedSession[]>(arKey, []).filter((a) => a.sessionId !== sessionId))
    },
    purgeArchived: async (before) => {
      const list = read<ArchivedSession[]>(arKey, [])
      write(arKey, list.filter((a) => a.archivedAt >= before))
      return list.filter((a) => a.archivedAt < before).map((a) => a.sessionId)
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
