import { isRemoteRuntime } from "../runtime";

let pendingBack: Promise<void> | undefined;

export function overlayBackPending() {
    return !!pendingBack;
}

export function afterOverlayBack(action: () => void) {
    if (pendingBack) void pendingBack.then(() => afterOverlayBack(action));
    else action();
}

export function popOverlayBack() {
    afterOverlayBack(() => {
        pendingBack = new Promise<void>((resolve) => {
            window.addEventListener(
                "popstate",
                () =>
                    setTimeout(() => {
                        pendingBack = undefined;
                        resolve();
                    }, 0),
                { once: true },
            );
        });
        history.back();
    });
}

export function bindOverlayBack(close: () => void) {
    if (!isRemoteRuntime()) return () => {};
    const id = crypto.randomUUID();
    let active = true;
    let bound = false;
    const back = () => {
        if (history.state?.driftOverlay !== id) close();
    };
    afterOverlayBack(() => {
        if (!active) return;
        history.pushState({ ...history.state, driftOverlay: id }, "");
        window.addEventListener("popstate", back);
        bound = true;
    });
    return () => {
        active = false;
        window.removeEventListener("popstate", back);
        if (bound && history.state?.driftOverlay === id) popOverlayBack();
    };
}
