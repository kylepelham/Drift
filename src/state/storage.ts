import { createSignal } from "solid-js"
import { shellInvoke } from "../shell"
import { persisted } from "./persist"

export type TableUsage = { table: string; rows: number; bytes: number }
export type SessionCounts = { total: number; topLevel: number; subagent: number; archived: number }
export type StorageStats = {
  path: string
  totalBytes: number
  freeBytes: number
  tables: TableUsage[]
  sessions: SessionCounts
  estimated: boolean
}
export type RuleEstimate = { rule: string; rows: number; bytes: number }
export type PruneResult = { removedRows: number; freedBytes: number; freeBytes: number }

/**
 * Which prune rules the user has enabled. These drive both the manual "Clean up now" action and the
 * scheduled cleanup, so the two can never disagree about what is safe to remove.
 */
export type StorageRules = {
  supersededSnapshots: boolean
  subagentEvents: boolean
  archivedEvents: boolean
  orphanEvents: boolean
}

/**
 * Defaults are the rules that cannot lose anything the user can read: superseded snapshots are
 * redundant copies, and orphaned events belong to sessions that no longer exist. Dropping a
 * subagent's or an archived session's log is defensible but is the user's call, so both start off.
 */
export const defaultStorageRules: StorageRules = {
  supersededSnapshots: true,
  subagentEvents: false,
  archivedEvents: false,
  orphanEvents: true,
}

export const [storageRules, setStorageRules] = persisted<StorageRules>("drift.storage.rules", defaultStorageRules)
export const [autoCleanup, setAutoCleanup] = persisted<boolean>("drift.storage.autoCleanup", false)
/** Epoch millis of the last automatic cleanup, so it runs at most once a day. */
export const [lastCleanupAt, setLastCleanupAt] = persisted<number>("drift.storage.lastCleanupAt", 0)

export function setStorageRule<K extends keyof StorageRules>(rule: K, enabled: boolean) {
  setStorageRules({ ...defaultStorageRules, ...storageRules(), [rule]: enabled })
}

export function anyRuleEnabled(rules: StorageRules) {
  return Object.values(rules).some(Boolean)
}

const [stats, setStats] = createSignal<StorageStats | null>(null)
const [estimates, setEstimates] = createSignal<RuleEstimate[] | null>(null)
const [busy, setBusy] = createSignal<"stats" | "analyze" | "prune" | "compact" | null>(null)
const [error, setError] = createSignal("")

export { stats as storageStats, estimates as storageEstimates, busy as storageBusy, error as storageError }

/** Runs a backend call, tracking which operation is in flight and surfacing its failure. */
async function run<T>(kind: NonNullable<ReturnType<typeof busy>>, command: string, args?: Record<string, unknown>) {
  const invoke = shellInvoke()
  if (!invoke) {
    setError("Storage management needs the desktop app")
    return undefined
  }
  setBusy(kind)
  setError("")
  try {
    return await invoke<T>(command, args)
  } catch (cause) {
    setError(cause instanceof Error ? cause.message : String(cause))
    return undefined
  } finally {
    setBusy(null)
  }
}

export async function refreshStorageStats() {
  const next = await run<StorageStats>("stats", "storage_stats")
  if (next) setStats(next)
}

/** Exact per-rule accounting. Slow: it scans the event table. */
export async function analyzeStorage() {
  const next = await run<RuleEstimate[]>("analyze", "storage_analyze")
  if (next) setEstimates(next)
}

export async function pruneStorage(rules = storageRules()) {
  const result = await run<PruneResult>("prune", "storage_prune", { rules })
  if (result) {
    // The estimates describe rows that no longer exist, so drop them rather than show stale figures.
    setEstimates(null)
    await refreshStorageStats()
  }
  return result
}

export async function compactStorage() {
  const result = await run<PruneResult>("compact", "storage_compact")
  if (result) await refreshStorageStats()
  return result
}

const dayMs = 24 * 60 * 60 * 1000

/**
 * Prunes at most once a day when automatic cleanup is on. Called from the same scheduler that purges
 * archived sessions, so cleanup happens on a timer rather than blocking startup.
 */
export async function runScheduledCleanup(now = Date.now()) {
  const rules = storageRules()
  if (!autoCleanup() || !anyRuleEnabled(rules)) return
  if (now - lastCleanupAt() < dayMs) return
  setLastCleanupAt(now)
  await pruneStorage(rules)
}

const units = ["B", "KB", "MB", "GB", "TB"]

/** Formats bytes for display, matching how the rest of the UI shows sizes. */
export function formatBytes(bytes: number) {
  if (bytes <= 0) return "0 B"
  const power = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)))
  const value = bytes / 1024 ** power
  return `${value.toFixed(power === 0 || value >= 100 ? 0 : 1)} ${units[power]}`
}
