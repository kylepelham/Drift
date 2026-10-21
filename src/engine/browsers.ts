import { produce, reconcile, type SetStoreFunction } from "solid-js/store";

import type { components } from "./native/types";
import type { EngineState } from "./store";

export type BrowserRecord = components["schemas"]["BrowserRecord"];
export type BrowserPage = components["schemas"]["PageRecord"];

/** When each session's last browser started, once it ended: a snapshot that raced its end never brings it back. */
const ended = new Map<string, number>();

/** Keeps a session's running browser, or drops it once it has stopped. */
export function putBrowser(set: SetStoreFunction<EngineState>, record: BrowserRecord) {
    if (record.state === "stopped") {
        ended.set(record.sessionId, record.startedAt);
        dropBrowser(set, record.sessionId);
        return;
    }

    const endedAt = ended.get(record.sessionId);
    if (endedAt !== undefined && endedAt >= record.startedAt) return;

    set("browsers", record.sessionId, reconcile(record));
}

export function dropBrowser(set: SetStoreFunction<EngineState>, sessionId: string) {
    set(
        "browsers",
        produce((browsers) => void delete browsers[sessionId]),
    );
}

/** The page the browser shows now. */
export function activePage(record: BrowserRecord | undefined): BrowserPage | undefined {
    return record?.pages.find((page) => page.active) ?? record?.pages.at(-1);
}
