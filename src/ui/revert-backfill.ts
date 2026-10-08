import { createEffect, createSignal, type Accessor } from "solid-js";
import { selectedSession } from "../state/selection";

import type { MessageEntry } from "../engine/store";
import type { Engine } from "../engine";

/**
 * Identifies one backfill attempt, so a page that failed is not requested again unchanged.
 *
 * A failed page leaves the cursor where it was, which is exactly the state that asked for the
 * page, so only a new session or a moved cursor is worth another request.
 */
export function revertBackfillAttempt(sessionId: string, cursor?: string | null) {
    return `${sessionId}\u0000${cursor ?? ""}`;
}

/** Whether an empty reverted timeline still has older pages that could reveal pre-revert rows. */
export function revertBackfillNeeded(input: {
    revertedAt?: string;
    visible: number;
    loaded?: boolean;
    cursor?: string | null;
}) {
    return !!input.revertedAt && input.visible === 0 && !!input.loaded && !!input.cursor;
}

/** Pages older history in while a revert hides every loaded message; true while a page is in flight. */
export function createRevertBackfill(engine: Engine, entries: Accessor<MessageEntry[]>) {
    // A revert that spans more than one transcript page can put every loaded message inside the
    // reverted range, leaving the timeline empty (the marker itself may not even be loaded). Page
    // older history in until something pre-revert is visible or the history is exhausted. The
    // in-flight signal re-runs this effect when each page lands, so the loop advances one page at
    // a time and stops the moment an entry survives the revert filter.
    const [revertBackfill, setRevertBackfill] = createSignal(false);
    const [revertBackfillFailure, setRevertBackfillFailure] = createSignal<string>();
    createEffect(() => {
        const id = selectedSession();
        if (!id || revertBackfill()) return;
        const cursor = engine.state.cursors[id];
        if (
            !revertBackfillNeeded({
                revertedAt: engine.state.sessions[id]?.revert?.messageId,
                visible: entries().length,
                loaded: engine.state.loaded[id],
                cursor,
            })
        )
            return;
        // A page that never arrived leaves the cursor untouched, so the next run would ask for the
        // same page and keep asking. Remember the attempt and wait for the cursor or session to move.
        const attempt = revertBackfillAttempt(id, cursor);
        if (revertBackfillFailure() === attempt) return;
        setRevertBackfill(true);
        void engine.actions
            .loadOlder(id)
            .then((loaded) => {
                if (!loaded) setRevertBackfillFailure(attempt);
            })
            .catch(() => setRevertBackfillFailure(attempt))
            .finally(() => setRevertBackfill(false));
    });

    return revertBackfill;
}
