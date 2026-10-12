/** Lets a finger drag the open mobile drawer shut: it follows a leftward drag and closes past a threshold or on a flick. */

/** Movement before a touch counts as a horizontal drag rather than a scroll. */
const decideAfter = 10;
/** The share of the drawer's width, or the speed in px/ms, that closes it on release. */
const closeShare = 1 / 3;
const flickSpeed = 0.5;

/** Whether a drag that ended `dx` px left of where it began, at `speed` px/ms, closes a drawer `width` px wide. */
export function swipeCloses(dx: number, width: number, speed: number) {
    return dx < 0 && (-dx > width * closeShare || -speed > flickSpeed);
}

/** Attaches the gesture to `drawer` while `open()`; returns the cleanup. */
export function swipeToClose(drawer: HTMLElement, open: () => boolean, close: () => void) {
    let start: { x: number; y: number; at: number } | null = null;
    let dragging = false;

    const reset = () => {
        drawer.style.removeProperty("transform");
        drawer.style.removeProperty("transition");
        start = null;
        dragging = false;
    };

    const onStart = (event: TouchEvent) => {
        const touch = event.touches[0];
        if (!open() || event.touches.length !== 1 || !touch) return;
        start = { x: touch.clientX, y: touch.clientY, at: event.timeStamp };
    };

    const onMove = (event: TouchEvent) => {
        const touch = event.touches[0];
        if (!start || !touch) return;
        const dx = touch.clientX - start.x;
        const dy = touch.clientY - start.y;

        // The first clear movement decides: up or down scrolls the list, left drags the drawer.
        if (!dragging) {
            if (Math.abs(dx) < decideAfter && Math.abs(dy) < decideAfter) return;
            if (Math.abs(dy) >= Math.abs(dx) || dx > 0) return void (start = null);
            dragging = true;
            drawer.style.transition = "none";
        }

        event.preventDefault();
        drawer.style.transform = `translateX(${Math.min(0, dx)}px)`;
    };

    const onEnd = (event: TouchEvent) => {
        const touch = event.changedTouches[0];
        if (!start || !dragging || !touch) return reset();

        const dx = touch.clientX - start.x;
        const speed = dx / Math.max(1, event.timeStamp - start.at);
        const closes = swipeCloses(dx, drawer.offsetWidth, speed);
        reset();
        if (closes) close();
    };

    drawer.addEventListener("touchstart", onStart, { passive: true });
    drawer.addEventListener("touchmove", onMove, { passive: false });
    drawer.addEventListener("touchend", onEnd);
    drawer.addEventListener("touchcancel", reset);

    return () => {
        drawer.removeEventListener("touchstart", onStart);
        drawer.removeEventListener("touchmove", onMove);
        drawer.removeEventListener("touchend", onEnd);
        drawer.removeEventListener("touchcancel", reset);
    };
}
