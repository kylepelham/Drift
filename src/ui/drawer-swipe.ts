/** Lets a finger drag the mobile drawer: rightward over the page opens it, leftward over the drawer or the page beside it closes it. */

/** Movement before a touch counts as a horizontal drag rather than a scroll. */
const decideAfter = 10;
/** The share of the drawer's width, or the speed in px/ms, that carries it the rest of the way on release. */
const settleShare = 1 / 3;
const flickSpeed = 0.5;

export type DrawerState = { open: () => boolean; show: () => void; hide: () => void; narrow: () => boolean };

/** Whether a drag that ended `dx` px from where it began, at `speed` px/ms, carries a drawer `width` px wide shut (`closing`) or open. */
export function swipeSettles(dx: number, width: number, speed: number, closing: boolean) {
    const along = closing ? -dx : dx;
    const pace = closing ? -speed : speed;

    return along > 0 && (along > width * settleShare || pace > flickSpeed);
}

/** Where a rightward drag belongs to something else: typing, a dialog or menu, or content that can scroll back. */
function swipeOpensFrom(target: EventTarget | null) {
    if (!(target instanceof Element)) return true;
    if (target.closest('input, textarea, [contenteditable="true"], [role="dialog"], [role="menu"], .pop-in'))
        return false;

    for (let node: Element | null = target; node; node = node.parentElement) {
        const sideways = node.scrollWidth > node.clientWidth && /auto|scroll/.test(getComputedStyle(node).overflowX);
        if (sideways && node.scrollLeft > 0) return false;
    }

    return true;
}

/** Attaches the gesture to the page for `drawer`; returns the cleanup. */
export function drawerSwipe(drawer: HTMLElement, state: DrawerState) {
    let start: { x: number; y: number; at: number; closing: boolean } | null = null;
    let dragging = false;

    const reset = () => {
        drawer.style.removeProperty("transform");
        drawer.style.removeProperty("transition");
        drawer.style.removeProperty("visibility");
        start = null;
        dragging = false;
    };

    const onStart = (event: TouchEvent) => {
        const touch = event.touches[0];
        if (!state.narrow() || event.touches.length !== 1 || !touch) return;

        const closing = state.open();
        if (!closing && !swipeOpensFrom(event.target)) return;
        start = { x: touch.clientX, y: touch.clientY, at: event.timeStamp, closing };
    };

    const onMove = (event: TouchEvent) => {
        const touch = event.touches[0];
        if (!start || !touch) return;
        const dx = touch.clientX - start.x;
        const dy = touch.clientY - start.y;

        // The first clear movement decides: up or down scrolls, the drawer's way drags it.
        if (!dragging) {
            if (Math.abs(dx) < decideAfter && Math.abs(dy) < decideAfter) return;
            const wrongWay = start.closing ? dx > 0 : dx < 0;
            if (Math.abs(dy) >= Math.abs(dx) || wrongWay) return void (start = null);
            dragging = true;
            drawer.style.transition = "none";
            drawer.style.visibility = "visible";
        }

        event.preventDefault();
        const offset = start.closing ? dx : dx - drawer.offsetWidth;
        drawer.style.transform = `translateX(${Math.min(0, offset)}px)`;
    };

    const onEnd = (event: TouchEvent) => {
        const touch = event.changedTouches[0];
        if (!start || !dragging || !touch) return reset();

        const { closing } = start;
        const dx = touch.clientX - start.x;
        const speed = dx / Math.max(1, event.timeStamp - start.at);
        const settles = swipeSettles(dx, drawer.offsetWidth, speed, closing);
        reset();
        if (settles && closing) state.hide();
        if (settles && !closing) state.show();
    };

    document.addEventListener("touchstart", onStart, { passive: true });
    document.addEventListener("touchmove", onMove, { passive: false });
    document.addEventListener("touchend", onEnd);
    document.addEventListener("touchcancel", reset);

    return () => {
        document.removeEventListener("touchstart", onStart);
        document.removeEventListener("touchmove", onMove);
        document.removeEventListener("touchend", onEnd);
        document.removeEventListener("touchcancel", reset);
    };
}
