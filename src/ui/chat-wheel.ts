export type ForwardedWheel = { deltaY: number; deltaMode: number };
export const chatWheelEvent = "drift:chat-wheel";

export function forwardWheelToChat(event: WheelEvent, boundary: HTMLElement) {
    if (event.ctrlKey || event.deltaY === 0 || wheelTargetConsumes(event.target, boundary, event.deltaY)) return false;

    window.dispatchEvent(
        new CustomEvent<ForwardedWheel>(chatWheelEvent, {
            detail: { deltaY: event.deltaY, deltaMode: event.deltaMode },
        }),
    );
    event.preventDefault();

    return true;
}

function wheelTargetConsumes(target: EventTarget | null, boundary: HTMLElement, deltaY: number) {
    let element = target instanceof Element ? target : null;
    while (element) {
        if (element.hasAttribute("data-wheel-lock")) return true;
        if (element !== boundary) {
            const style = getComputedStyle(element);
            const scrollable = style.overflowY === "auto" || style.overflowY === "scroll";
            if (scrollable && element.scrollHeight > element.clientHeight) {
                const remaining = element.scrollHeight - element.clientHeight - element.scrollTop;
                if ((deltaY < 0 && element.scrollTop > 0) || (deltaY > 0 && remaining > 1)) return true;
            }
        }
        if (element === boundary) break;
        element = element.parentElement;
    }
    return false;
}

export function normalizedWheelDelta(deltaY: number, deltaMode: number, viewportHeight: number) {
    if (deltaMode === 1) return deltaY * 16;
    if (deltaMode === 2) return deltaY * viewportHeight;
    return deltaY;
}

export function accumulatedWheelTarget(scrollTop: number, pendingTarget: number | null, delta: number, max: number) {
    return Math.min(max, Math.max(0, (pendingTarget ?? scrollTop) + delta));
}

/** A notch from a mouse wheel is animated; a touchpad's small, frequent deltas already are, so they apply at once. */
export const wheelNotch = 50;
/** How quickly the animation closes on its target, as Chromium's own wheel smoothing feels. */
const wheelEaseMs = 70;

/** Where the animated position is after `elapsed` ms: most of the way there each frame, exactly there once within half a pixel. */
export function wheelStep(position: number, target: number, elapsed: number) {
    const next = position + (target - position) * (1 - Math.exp(-elapsed / wheelEaseMs));
    return Math.abs(target - next) < 0.5 ? target : next;
}
