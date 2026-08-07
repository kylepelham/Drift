import { createSignal } from "solid-js"
import type { EngineError, RecoverableError } from "../engine/error"
import type { ModelRef } from "../engine/store"
import { driftStore, type DriftStore, type RecoverableInterruption, type Workspace } from "./store"

const [recoverableInterruptions, setRecoverableInterruptions] = createSignal<RecoverableInterruption[]>([])
const [resumedSessions, setResumedSessions] = createSignal<ReadonlySet<string>>(new Set())
let initialized = false
const pendingClears = new Set<string>()
let persistence = Promise.resolve()

export { recoverableInterruptions, resumedSessions }

export async function initRecoverableInterruptions(store: DriftStore = driftStore) {
  const saved = await store.interruptions()
  setRecoverableInterruptions((current) =>
    mergeInterruptions(saved.filter((item) => !pendingClears.has(item.sessionId)), current),
  )
  initialized = true
  pendingClears.clear()
}

export function recoverableForSession(sessionId: string) {
  return recoverableInterruptions()
    .filter((item) => item.sessionId === sessionId)
    .sort((a, b) => b.updatedAt - a.updatedAt)[0]
}

export function interruptionIdentity(error: EngineError, model?: ModelRef, messageId?: string) {
  if (messageId) return messageId
  const data = error.data && typeof error.data === "object" ? (error.data as Record<string, unknown>) : {}
  return [error.name ?? "Error", data.statusCode ?? "", model?.providerID ?? "", model?.modelID ?? "", data.message ?? ""]
    .join("\0")
    .slice(0, 1000)
}

export function recordRecoverableInterruption(
  input: Omit<RecoverableInterruption, "kind" | "reason" | "createdAt" | "updatedAt" | "dismissedAt"> &
    RecoverableError,
  store: DriftStore = driftStore,
) {
  const timestamp = Date.now()
  let saved!: RecoverableInterruption
  setRecoverableInterruptions((current) => {
    const existing = current.find((item) => item.sessionId === input.sessionId && item.identity === input.identity)
    saved = {
      ...input,
      createdAt: existing?.createdAt ?? timestamp,
      updatedAt: timestamp,
      dismissedAt: existing?.dismissedAt,
    }
    return [...current.filter((item) => item.sessionId !== saved.sessionId), saved]
  })
  setResumedSessions((current) => {
    if (!current.has(input.sessionId)) return current
    const next = new Set(current)
    next.delete(input.sessionId)
    return next
  })
  persist(() => store.saveInterruption(saved))
  return saved
}

export function updateRecoverableFailure(sessionId: string, reason: string, model: ModelRef, store: DriftStore = driftStore) {
  const current = recoverableForSession(sessionId)
  if (!current) return
  const updated = {
    ...current,
    providerId: model.providerID,
    modelId: model.modelID,
    reason,
    updatedAt: Date.now(),
  }
  setRecoverableInterruptions((items) => items.map((item) => (item === current ? updated : item)))
  persist(() => store.saveInterruption(updated))
}

export function dismissRecoverableInterruption(
  sessionId: string,
  identity: string,
  store: DriftStore = driftStore,
) {
  const dismissedAt = Date.now()
  setRecoverableInterruptions((items) =>
    items.map((item) =>
      item.sessionId === sessionId && item.identity === identity ? { ...item, dismissedAt } : item,
    ),
  )
  persist(() => store.dismissInterruption(sessionId, identity, dismissedAt))
}

export function clearRecoverableInterruption(sessionId: string, resumed = false, store: DriftStore = driftStore) {
  const hadInterruption = recoverableInterruptions().some((item) => item.sessionId === sessionId)
  if (hadInterruption) setRecoverableInterruptions((items) => items.filter((item) => item.sessionId !== sessionId))
  setResumedSessions((current) => {
    if (resumed && hadInterruption) return new Set([...current, sessionId])
    if (!current.has(sessionId)) return current
    const next = new Set(current)
    next.delete(sessionId)
    return next
  })
  if (initialized && !hadInterruption) return
  if (!initialized) pendingClears.add(sessionId)
  persist(() => store.clearInterruptions(sessionId))
}

export function recoveryNavigationTarget(interruption: RecoverableInterruption, workspaces: Workspace[]) {
  const canonical = interruption.directory.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase()
  const workspace = workspaces.find((item) => item.id === interruption.workspaceId) ??
    workspaces.find((item) => item.path.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase() === canonical)
  return { workspaceId: workspace?.id, sessionId: interruption.sessionId }
}

export function mergeInterruptions(saved: RecoverableInterruption[], current: RecoverableInterruption[]) {
  const entries = new Map(saved.map((item) => [`${item.sessionId}\0${item.identity}`, item]))
  for (const item of current) entries.set(`${item.sessionId}\0${item.identity}`, item)
  return [...entries.values()]
}

function persist(operation: () => Promise<unknown>) {
  persistence = persistence.then(operation, operation).then(
    () => undefined,
    () => undefined,
  )
}
