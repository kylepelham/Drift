import { createSignal } from "solid-js"
import { isRemoteRuntime } from "../runtime"
import { parseNavigationHash, pushRemoteSelection } from "./navigation"
import { selectSession } from "./selection"
import { persisted } from "./persist"
import { driftStore, type ArchivedSession, type Workspace } from "./store"

const [rawWorkspaces, setWorkspaces] = createSignal<Workspace[]>([])
const [workspacesReady, setWorkspacesReady] = createSignal(false)
const [archivedIds, setArchivedIds] = createSignal<ReadonlySet<string>>(new Set())
const [archivedSessions, setArchivedSessions] = createSignal<ArchivedSession[]>([])
const [removedWorkspaces, setRemovedWorkspaces] = createSignal<Workspace[]>([])
const [activeWorkspaceId, setActiveWorkspaceId] = persisted<string | null>("drift.workspace", null)
const [workspaceOrder, setWorkspaceOrder] = persisted<string[]>("drift.workspace.order", [])
const [collapsedWorkspaceIds, setCollapsedWorkspaceIds] = persisted<string[]>("drift.workspace.collapsed", [])

export { archivedIds, archivedSessions, removedWorkspaces, activeWorkspaceId, workspacesReady }

export function workspaces() {
  const order = workspaceOrder()
  const rank = (w: Workspace) => {
    const index = order.indexOf(w.id)
    return index < 0 ? order.length : index
  }
  return [...rawWorkspaces()].sort((a, b) => rank(a) - rank(b))
}

export function moveWorkspace(id: string, beforeId: string | null) {
  const ids = workspaces().map((w) => w.id).filter((x) => x !== id)
  const index = beforeId ? ids.indexOf(beforeId) : -1
  ids.splice(index < 0 ? ids.length : index, 0, id)
  setWorkspaceOrder(ids)
}

export function activeWorkspace() {
  return rawWorkspaces().find((w) => w.id === activeWorkspaceId()) ?? null
}

export function workspaceCollapsed(id: string) {
  return collapsedWorkspaceIds().includes(id)
}

export function nextCollapsedWorkspaceIds(current: string[], id: string) {
  return current.includes(id) ? current.filter((item) => item !== id) : [...current, id]
}

export function toggleWorkspaceCollapsed(id: string) {
  setCollapsedWorkspaceIds(nextCollapsedWorkspaceIds(collapsedWorkspaceIds(), id))
}

export async function initWorkspaces() {
  try {
    await refreshWorkspaces()
  } finally {
    setWorkspacesReady(true)
  }
  await refreshArchives()
}

async function refreshWorkspaces() {
  const [active, removed] = await Promise.all([driftStore.workspaces(), driftStore.removedWorkspaces()])
  setWorkspaces(active)
  setRemovedWorkspaces(removed)
  const ids = active.map((w) => w.id)
  const kept = workspaceOrder().filter((id) => ids.includes(id))
  const merged = [...kept, ...ids.filter((id) => !kept.includes(id))]
  if (merged.join(",") !== workspaceOrder().join(",")) setWorkspaceOrder(merged)
}

async function refreshArchives() {
  const sessions = await driftStore.archived()
  setArchivedSessions(sessions)
  setArchivedIds(new Set(sessions.map((entry) => entry.sessionId)))
}

export function selectWorkspace(id: string) {
  if (activeWorkspaceId() !== id) selectSession(null, false)
  setActiveWorkspaceId(id)
  pushRemoteSelection({ workspace: id, session: undefined })
  void driftStore.touchWorkspace(id)
}

export async function addWorkspace(path: string) {
  const name = path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1) ?? path
  const workspace = await driftStore.addWorkspace({ id: crypto.randomUUID(), path, name, icon: "" })
  await refreshWorkspaces()
  selectWorkspace(workspace.id)
}

if (typeof window !== "undefined" && isRemoteRuntime()) {
  window.addEventListener("popstate", () => {
    const workspace = parseNavigationHash(window.location.hash).workspace
    if (workspace) setActiveWorkspaceId(workspace)
  })
}

export async function updateWorkspace(id: string, patch: { path?: string; name?: string; icon?: string }) {
  const workspace = workspaces().find((w) => w.id === id)
  if (!workspace) return
  await driftStore.saveWorkspace({
    id,
    path: patch.path ?? workspace.path,
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

// Both purges are two-phase: the Drift record is the deletion tombstone and is only dropped once
// the engine confirms every session is gone. A failed engine deletion keeps the tombstone, so the
// purge resumes on the next startup, reconnect, or timer tick. `true` means nothing is pending.
export async function purgeArchived(removeSession: (sessionId: string) => Promise<boolean>) {
  const expired = await driftStore.expiredArchived(Date.now() - purgeAge)
  let complete = true
  for (const sessionId of expired) {
    if (await removeSession(sessionId)) await driftStore.unarchiveSession(sessionId)
    else complete = false
  }
  await refreshArchives()
  return complete
}

export async function purgeRemovedWorkspaces(
  removeSessions: (directory: string, eligible: () => boolean) => Promise<boolean>,
) {
  const expired = await driftStore.expiredRemovedWorkspaces(Date.now() - purgeAge)
  const canonical = (path: string) => path.replaceAll("\\", "/").toLowerCase()
  let complete = true
  for (const workspace of expired) {
    // Never delete sessions in a directory that is on the sidebar or restored mid-drain.
    const eligible = () =>
      !rawWorkspaces().some((current) => canonical(current.path) === canonical(workspace.path)) &&
      !removedWorkspaces().some(
        (current) => current.id === workspace.id && (current.removedAt ?? 0) > (workspace.removedAt ?? 0),
      )
    if (!(await removeSessions(workspace.path, eligible))) {
      complete = false
      continue
    }
    await driftStore.forgetWorkspace(workspace.id)
  }
  await refreshWorkspaces()
  return complete
}

export async function purgeAll(engine: {
  purgeSession: (sessionId: string) => Promise<boolean>
  removeAllSessions: (directory: string, eligible: () => boolean) => Promise<boolean>
}) {
  const [archived, removed] = await Promise.all([
    purgeArchived(engine.purgeSession).catch(() => false),
    purgeRemovedWorkspaces(engine.removeAllSessions).catch(() => false),
  ])
  return archived && removed
}
