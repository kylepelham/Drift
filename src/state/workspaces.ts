import { createSignal } from "solid-js"
import { selectSession } from "./selection"
import { persisted } from "./persist"
import { driftStore, type ArchivedSession, type Workspace } from "./store"

const [workspaces, setWorkspaces] = createSignal<Workspace[]>([])
const [archivedIds, setArchivedIds] = createSignal<ReadonlySet<string>>(new Set())
const [archivedSessions, setArchivedSessions] = createSignal<ArchivedSession[]>([])
const [removedWorkspaces, setRemovedWorkspaces] = createSignal<Workspace[]>([])
const [activeWorkspaceId, setActiveWorkspaceId] = persisted<string | null>("drift.workspace", null)

export { workspaces, archivedIds, archivedSessions, removedWorkspaces, activeWorkspaceId }

export function activeWorkspace() {
  return workspaces().find((w) => w.id === activeWorkspaceId()) ?? null
}

export async function initWorkspaces() {
  await refreshWorkspaces()
  await refreshArchives()
}

async function refreshWorkspaces() {
  const [active, removed] = await Promise.all([driftStore.workspaces(), driftStore.removedWorkspaces()])
  setWorkspaces(active)
  setRemovedWorkspaces(removed)
}

async function refreshArchives() {
  const sessions = await driftStore.archived()
  setArchivedSessions(sessions)
  setArchivedIds(new Set(sessions.map((entry) => entry.sessionId)))
}

export function selectWorkspace(id: string) {
  if (activeWorkspaceId() !== id) selectSession(null)
  setActiveWorkspaceId(id)
  void driftStore.touchWorkspace(id)
}

export async function addWorkspace(path: string) {
  const name = path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1) ?? path
  const workspace = await driftStore.addWorkspace({ id: crypto.randomUUID(), path, name, icon: "" })
  await refreshWorkspaces()
  selectWorkspace(workspace.id)
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
  await refreshWorkspaces()
}

export async function removeWorkspace(id: string) {
  await driftStore.removeWorkspace(id)
  await refreshWorkspaces()
  if (activeWorkspaceId() === id) {
    setActiveWorkspaceId(null)
    selectSession(null)
  }
}

export async function restoreWorkspace(workspace: Workspace) {
  const restored = await driftStore.addWorkspace({
    id: workspace.id,
    path: workspace.path,
    name: workspace.name,
    icon: workspace.icon,
  })
  await refreshWorkspaces()
  selectWorkspace(restored.id)
}

export async function archiveSession(sessionId: string, workspaceId: string) {
  await driftStore.archiveSession(sessionId, workspaceId)
  await refreshArchives()
}

export async function unarchiveSession(sessionId: string) {
  await driftStore.unarchiveSession(sessionId)
  await refreshArchives()
}

const purgeAge = 7 * 24 * 60 * 60 * 1000

export async function purgeArchived() {
  const ids = await driftStore.purgeArchived(Date.now() - purgeAge)
  await refreshArchives()
  return ids
}

export async function purgeRemovedWorkspaces() {
  const paths = await driftStore.purgeRemovedWorkspaces(Date.now() - purgeAge)
  await refreshWorkspaces()
  return paths
}
