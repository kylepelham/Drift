import { createSignal } from "solid-js"
import { selectSession } from "./selection"
import { persisted } from "./persist"
import { driftStore, type Workspace } from "./store"

const [workspaces, setWorkspaces] = createSignal<Workspace[]>([])
const [archivedIds, setArchivedIds] = createSignal<ReadonlySet<string>>(new Set())
const [activeWorkspaceId, setActiveWorkspaceId] = persisted<string | null>("drift.workspace", null)

export { workspaces, archivedIds, activeWorkspaceId }

export function activeWorkspace() {
  return workspaces().find((w) => w.id === activeWorkspaceId()) ?? null
}

export async function initWorkspaces() {
  setWorkspaces(await driftStore.workspaces())
  setArchivedIds(new Set((await driftStore.archived()).map((a) => a.sessionId)))
}

export function selectWorkspace(id: string) {
  if (activeWorkspaceId() !== id) selectSession(null)
  setActiveWorkspaceId(id)
  void driftStore.touchWorkspace(id)
}

export async function addWorkspace(path: string) {
  const existing = workspaces().find((w) => w.path === path)
  if (existing) return selectWorkspace(existing.id)
  const id = crypto.randomUUID()
  const name = path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1) ?? path
  await driftStore.saveWorkspace({ id, path, name, icon: "" })
  setWorkspaces(await driftStore.workspaces())
  selectWorkspace(id)
}

export async function updateWorkspace(id: string, patch: { name?: string; icon?: string }) {
  const workspace = workspaces().find((w) => w.id === id)
  if (!workspace) return
  await driftStore.saveWorkspace({
    id,
    path: workspace.path,
    name: patch.name ?? workspace.name,
    icon: patch.icon ?? workspace.icon,
  })
  setWorkspaces(await driftStore.workspaces())
}

export async function removeWorkspace(id: string) {
  await driftStore.deleteWorkspace(id)
  setWorkspaces(await driftStore.workspaces())
  if (activeWorkspaceId() === id) {
    setActiveWorkspaceId(null)
    selectSession(null)
  }
}

export async function archiveSession(sessionId: string, workspaceId: string) {
  await driftStore.archiveSession(sessionId, workspaceId)
  setArchivedIds(new Set([...archivedIds(), sessionId]))
}

const purgeAge = 7 * 24 * 60 * 60 * 1000

export function purgeArchived() {
  return driftStore.purgeArchived(Date.now() - purgeAge)
}
