import type { Envelope } from "./native/client";

type PartDelta = Extract<Envelope, { type: "part.delta" }>;

/** How long a streamed token may wait before it is shown, folded with the ones after it. */
const DELTA_HOLD_MS = 100;

/** Holds part deltas briefly, folded per part; any other event flushes them first, keeping the order. */
export function createDeltaBuffer(apply: (envelope: Envelope) => void, hold = DELTA_HOLD_MS) {
    let held: PartDelta[] = [];
    let timer: ReturnType<typeof setTimeout> | undefined;

    const flush = () => {
        clearTimeout(timer);
        timer = undefined;
        const folded = foldDeltas(held);
        held = [];
        for (const delta of folded) apply(delta);
    };

    return {
        push(envelope: Envelope) {
            if (envelope.type !== "part.delta") {
                flush();
                apply(envelope);
                return;
            }
            held.push(envelope);
            timer ??= setTimeout(flush, hold);
        },
        flush,
    };
}

/** Joins a delta onto the one before it when it continues the same part exactly where that one ended. */
export function foldDeltas(deltas: PartDelta[]) {
    const folded: PartDelta[] = [];
    for (const delta of deltas) {
        const last = folded.at(-1);
        if (last && last.partId === delta.partId && delta.offset === last.offset + last.delta.length) {
            folded[folded.length - 1] = { ...last, delta: last.delta + delta.delta, seq: delta.seq };
            continue;
        }
        folded.push(delta);
    }

    return folded;
}
