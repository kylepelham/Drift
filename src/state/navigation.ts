import { afterOverlayBack, overlayBackPending, popOverlayBack } from "./overlay-back";
import { isRemoteRuntime } from "../runtime";
import { createSignal } from "solid-js";

const mobileBreakpoint = 720;
export function isNarrowWidth(width: number) {
    return width < mobileBreakpoint;
}

export type RemoteRoute = { workspace?: string; session?: string; overlay?: "drawer" | "settings" };

export function navigationHash(route: RemoteRoute) {
    const params = new URLSearchParams();
    if (route.workspace) params.set("workspace", route.workspace);
    if (route.session) params.set("session", route.session);
    if (route.overlay) params.set("overlay", route.overlay);

    return `#/${params.toString()}`;
}

export function parseNavigationHash(hash: string): RemoteRoute {
    const params = new URLSearchParams(hash.replace(/^#\/?/, ""));
    const overlay = params.get("overlay");

    return {
        workspace: params.get("workspace") || undefined,
        session: params.get("session") || undefined,
        overlay: overlay === "drawer" || overlay === "settings" ? overlay : undefined,
    };
}

function currentRoute() {
    return parseNavigationHash(window.location.hash);
}

export function pushRemoteSelection(patch: Pick<RemoteRoute, "workspace" | "session">) {
    if (!isRemoteRuntime()) return;

    afterOverlayBack(() => {
        const next = { ...currentRoute(), ...patch, overlay: undefined };
        history.pushState(null, "", navigationHash(next));
    });
}

/**
 * Puts a mirrored remote selection in the address bar without adding a history entry.
 * @public loaded by main.tsx through a dynamic import, which knip cannot follow
 */
export function replaceRemoteSelection(workspace: string | null, session: string | null) {
    if (!isRemoteRuntime()) return;

    afterOverlayBack(() =>
        history.replaceState(
            history.state,
            "",
            navigationHash({ ...currentRoute(), workspace: workspace ?? undefined, session: session ?? undefined }),
        ),
    );
}

export function pushRemoteOverlay(overlay: RemoteRoute["overlay"]) {
    if (!isRemoteRuntime()) return;

    afterOverlayBack(() => {
        const route = currentRoute();
        if (route.overlay === overlay) return;
        const next = navigationHash({ ...route, overlay });
        if (route.overlay) history.replaceState(null, "", next);
        else history.pushState(null, "", next);
    });
}

const [mobileDrawerOpen, setMobileDrawerOpen] = createSignal(false);
export { mobileDrawerOpen };

export function openMobileDrawer() {
    if (mobileDrawerOpen()) return;
    afterOverlayBack(() => {
        setMobileDrawerOpen(true);
        pushRemoteOverlay("drawer");
    });
}

export function closeMobileDrawer(fromHistory = false) {
    setMobileDrawerOpen(false);
    if (!fromHistory && isRemoteRuntime() && currentRoute().overlay === "drawer") popOverlayBack();
}

if (typeof window !== "undefined") {
    window.addEventListener("popstate", () => {
        if (!overlayBackPending()) setMobileDrawerOpen(currentRoute().overlay === "drawer");
    });
}
