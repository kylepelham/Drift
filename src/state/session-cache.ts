import { persisted } from "./persist";

/** The sidebar rows a workspace can render without the engine. */
export type CachedSession = { id: string; title: string; updated: number };

/** Retains several five-thread pages per workspace without making the persisted cache large. */
export const cachedSessionLimit = 30;

export function normalizeSessionCache(value: unknown): Record<string, CachedSession[]> {
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const result: Record<string, CachedSession[]> = {};
    for (const [directory, sessions] of Object.entries(value as Record<string, unknown>)) {
        if (!Array.isArray(sessions)) continue;
        const rows = sessions.flatMap((session) => {
            if (!session || typeof session !== "object") return [];
            const record = session as Record<string, unknown>;
            if (typeof record.id !== "string" || typeof record.title !== "string") return [];
            if (typeof record.updated !== "number" || !Number.isFinite(record.updated)) return [];
            return [{ id: record.id, title: record.title, updated: record.updated }];
        });
        if (rows.length) result[directory] = rows.slice(0, cachedSessionLimit);
    }
    return result;
}

const [cache, setCache] = persisted<Record<string, CachedSession[]>>("drift.sessions.cache", {}, normalizeSessionCache);

/** Returns the last known sidebar rows while the engine starts. */
export function cachedSessions(directory: string) {
    return cache()[directory] ?? [];
}

function sessionCacheSignature(sessions: CachedSession[]) {
    return sessions.map((session) => `${session.id}\u0000${session.title}\u0000${session.updated}`).join("\u0001");
}

/** Records the authoritative list, skipping the write when nothing the sidebar shows changed. */
export function rememberSessions(directory: string, sessions: CachedSession[]) {
    const next = sessions.slice(0, cachedSessionLimit);
    const current = cache()[directory];
    if (current && sessionCacheSignature(current) === sessionCacheSignature(next)) return;
    if (!next.length && !current) return;
    const updated = { ...cache() };
    if (next.length) updated[directory] = next;
    else delete updated[directory];
    setCache(updated);
}

export function forgetCachedSessions(directory: string) {
    if (!(directory in cache())) return;
    const updated = { ...cache() };
    delete updated[directory];
    setCache(updated);
}
