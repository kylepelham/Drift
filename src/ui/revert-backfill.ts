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
    // Each landed page re-runs this, so history pages in one at a time until a pre-revert row shows.
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
        // A failed page leaves the cursor unchanged; wait for it or the session to move.
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
