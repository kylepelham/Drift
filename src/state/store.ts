export type Workspace = { id: string; path: string; name: string; icon: string; lastUsed: number }
export type ArchivedSession = { sessionId: string; workspaceId: string; archivedAt: number }

export interface DriftStore {
  workspaces(): Promise<Workspace[]>
  addWorkspace(workspace: Omit<Workspace, "lastUsed">): Promise<Workspace>
  saveWorkspace(workspace: Omit<Workspace, "lastUsed">): Promise<void>
  touchWorkspace(id: string): Promise<void>
  removeWorkspace(id: string): Promise<void>
  purgeRemovedWorkspaces(before: number): Promise<string[]>
  archived(): Promise<ArchivedSession[]>
  archiveSession(sessionId: string, workspaceId: string): Promise<void>
  unarchiveSession(sessionId: string): Promise<void>
  purgeArchived(before: number): Promise<string[]>
}

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

export function shellInvoke(): Invoke | undefined {
  return (globalThis as { __TAURI__?: { core?: { invoke: Invoke } } }).__TAURI__?.core?.invoke
}

function shellStore(invoke: Invoke): DriftStore {
  return {
    workspaces: () => invoke("store_workspaces"),
    addWorkspace: (w) => invoke("store_add_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    saveWorkspace: (w) => invoke("store_save_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    touchWorkspace: (id) => invoke("store_touch_workspace", { id }),
    removeWorkspace: (id) => invoke("store_remove_workspace", { id }),
    purgeRemovedWorkspaces: (before) => invoke("store_purge_removed_workspaces", { before }),
    archived: () => invoke("store_archived"),
    archiveSession: (sessionId, workspaceId) => invoke("store_archive_session", { sessionId, workspaceId }),
    unarchiveSession: (sessionId) => invoke("store_unarchive_session", { sessionId }),
    purgeArchived: (before) => invoke("store_purge_archived", { before }),
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
  return {
    workspaces: async () => all().filter((w) => !w.removedAt).sort((a, b) => b.lastUsed - a.lastUsed),
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
      write(wsKey, all().map((x) => (x.id === w.id ? { ...x, name: w.name, icon: w.icon } : x)))
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
  }
}

const invoke = shellInvoke()
export const driftStore: DriftStore = invoke ? shellStore(invoke) : browserStore()
