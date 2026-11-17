import { createSignal } from "solid-js";

/** The message the message shortcuts last moved to, ringed in the transcript until Escape or another chat. */
const [messageCursor, setMessageCursor] = createSignal<string | null>(null);

export { messageCursor, setMessageCursor };

export function clearMessageCursor() {
    setMessageCursor(null);
}

/** The next message one `step` from the cursor that `eligible` takes; with no cursor, from `start`, the one in view. */
export function stepMessageCursor(
    order: string[],
    current: string | null,
    step: 1 | -1,
    start: string | undefined,
    eligible: (id: string) => boolean = () => true,
) {
    const from = current ? order.indexOf(current) : -1;
    const origin = start ? order.indexOf(start) : -1;
    let index = from >= 0 ? from + step : origin;
    if (from < 0 && origin < 0) index = step < 0 ? order.length - 1 : 0;
    for (; index >= 0 && index < order.length; index += step) if (eligible(order[index])) return order[index];

    return undefined;
}
