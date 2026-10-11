import type { SetStoreFunction } from "solid-js/store";
import type { components } from "./native/types";
import type { EngineState } from "./store";

export type ProcessRecord = components["schemas"]["ProcessRecord"];

/** An ended record outranks a running one, so a snapshot that raced an event never brings it back to life. */
function progress(record: ProcessRecord) {
    return record.state === "running" ? 0 : 1;
}

/** Folds process records into a conversation's list, oldest first. */
export function mergeProcesses(current: readonly ProcessRecord[] | undefined, incoming: readonly ProcessRecord[]) {
    const byId = new Map((current ?? []).map((record) => [record.id, record]));
    for (const record of incoming) {
        const known = byId.get(record.id);
        if (!known || progress(record) >= progress(known)) byId.set(record.id, record);
    }

    return [...byId.values()].sort((a, b) => a.startedAt - b.startedAt || a.id.localeCompare(b.id));
}

export function putProcesses(
    set: SetStoreFunction<EngineState>,
    state: EngineState,
    sessionId: string,
    records: readonly ProcessRecord[],
) {
    set("processes", sessionId, mergeProcesses(state.processes[sessionId], records));
}
