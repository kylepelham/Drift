import { backendInvoke } from "../backend";
import { createSignal } from "solid-js";

type TableUsage = { table: string; rows: number; bytes: number };
type SessionCounts = { total: number; topLevel: number; subagent: number; archived: number };
export type StorageStats = {
    path: string;
    totalBytes: number;
    freeBytes: number;
    tables: TableUsage[];
    sessions: SessionCounts;
    estimated: boolean;
};
export type PruneResult = { removedRows: number; freedBytes: number; freeBytes: number };

const [stats, setStats] = createSignal<StorageStats | null>(null);
const [busy, setBusy] = createSignal<"stats" | "prune" | "compact" | null>(null);
const [error, setError] = createSignal("");

export { stats as storageStats, busy as storageBusy, error as storageError };

/** Runs a backend call, tracking which operation is in flight and surfacing its failure. */
async function run<T>(kind: NonNullable<ReturnType<typeof busy>>, command: string) {
    const invoke = backendInvoke();
    if (!invoke) {
        setError("Storage management needs the Drift host backend");
        return undefined;
    }
    setBusy(kind);
    setError("");
    try {
        return await invoke<T>(command);
    } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        return undefined;
    } finally {
        setBusy(null);
    }
}

export async function refreshStorageStats() {
    const next = await run<StorageStats>("stats", "storage_stats");
    if (next) setStats(next);
}

/** The engine's own housekeeping, now rather than at its next run (it runs every few hours). */
export async function pruneStorage() {
    const result = await run<PruneResult>("prune", "storage_prune");
    if (result) await refreshStorageStats();
    return result;
}

export async function compactStorage() {
    const result = await run<PruneResult>("compact", "storage_compact");
    if (result) await refreshStorageStats();
    return result;
}

const units = ["B", "KB", "MB", "GB", "TB"];

/** Formats bytes for display, matching how the rest of the UI shows sizes. */
export function formatBytes(bytes: number) {
    if (bytes <= 0) return "0 B";
    const power = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
    const value = bytes / 1024 ** power;
    return `${value.toFixed(power === 0 || value >= 100 ? 0 : 1)} ${units[power]}`;
}
