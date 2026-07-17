export type Workspace = { id: string; path: string; name: string; icon: string; lastUsed: number }
export type ArchivedSession = { sessionId: string; workspaceId: string; archivedAt: number }

export interface DriftStore {
  workspaces(): Promise<Workspace[]>
  saveWorkspace(workspace: Omit<Workspace, "lastUsed">): Promise<void>
  touchWorkspace(id: string): Promise<void>
  deleteWorkspace(id: string): Promise<void>
  archived(): Promise<ArchivedSession[]>
  archiveSession(sessionId: string, workspaceId: string): Promise<void>
  purgeArchived(before: number): Promise<string[]>
}

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>

export function shellInvoke(): Invoke | undefined {
  return (globalThis as { __TAURI__?: { core?: { invoke: Invoke } } }).__TAURI__?.core?.invoke
}

function shellStore(invoke: Invoke): DriftStore {
  return {
    workspaces: () => invoke("store_workspaces"),
    saveWorkspace: (w) => invoke("store_save_workspace", { id: w.id, path: w.path, name: w.name, icon: w.icon }),
    touchWorkspace: (id) => invoke("store_touch_workspace", { id }),
    deleteWorkspace: (id) => invoke("store_delete_workspace", { id }),
    archived: () => invoke("store_archived"),
    archiveSession: (sessionId, workspaceId) => invoke("store_archive_session", { sessionId, workspaceId }),
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

function browserStore(): DriftStore {
  const wsKey = "drift.store.workspaces"
  const arKey = "drift.store.archived"
  return {
    workspaces: async () => read<Workspace[]>(wsKey, []).sort((a, b) => b.lastUsed - a.lastUsed),
    saveWorkspace: async (w) => {
      const list = read<Workspace[]>(wsKey, []).filter((x) => x.id !== w.id)
      write(wsKey, [...list, { ...w, lastUsed: Date.now() }])
    },
    touchWorkspace: async (id) => {
      write(wsKey, read<Workspace[]>(wsKey, []).map((w) => (w.id === id ? { ...w, lastUsed: Date.now() } : w)))
    },
    deleteWorkspace: async (id) => {
      write(wsKey, read<Workspace[]>(wsKey, []).filter((w) => w.id !== id))
      write(arKey, read<ArchivedSession[]>(arKey, []).filter((a) => a.workspaceId !== id))
    },
    archived: async () => read<ArchivedSession[]>(arKey, []),
    archiveSession: async (sessionId, workspaceId) => {
      const list = read<ArchivedSession[]>(arKey, []).filter((a) => a.sessionId !== sessionId)
      write(arKey, [...list, { sessionId, workspaceId, archivedAt: Date.now() }])
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
