/** A wheel over a menu marked `data-wheel-lock` never scrolls the transcript behind it, only the menu's own lists. */
export function holdWheelInLock(event: WheelEvent) {
    const target = event.target instanceof Element ? event.target : null;
    const lock = target?.closest("[data-wheel-lock]");
    if (!lock || event.ctrlKey || scrollsWithin(target!, lock, event.deltaY)) return;

    event.preventDefault();
}

function scrollsWithin(target: Element, lock: Element, deltaY: number) {
    for (let element: Element | null = target; element; element = element.parentElement) {
        const style = getComputedStyle(element);
        const scrollable = style.overflowY === "auto" || style.overflowY === "scroll";
        const remaining = element.scrollHeight - element.clientHeight - element.scrollTop;
        if (scrollable && ((deltaY < 0 && element.scrollTop > 0) || (deltaY > 0 && remaining > 1))) return true;
        if (element === lock) return false;
    }

    return false;
}
