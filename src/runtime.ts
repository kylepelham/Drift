export type RuntimeLocation = Pick<Location, "pathname" | "origin">;

/** The Drift app on a phone: its local proxy serves the UI under `/app` and stands in for a PC's gateway. */
export function appRuntimeFrom(location: RuntimeLocation) {
    return location.pathname === "/app" || location.pathname.startsWith("/app/");
}

export function remoteRuntimeFrom(location: RuntimeLocation) {
    const companion = location.pathname === "/companion" || location.pathname.startsWith("/companion/");
    return companion || appRuntimeFrom(location);
}

export function runtimeNameFrom(location: RuntimeLocation) {
    return remoteRuntimeFrom(location) ? "remote" : "desktop";
}

export function isRemoteRuntime() {
    return typeof window !== "undefined" && remoteRuntimeFrom(window.location);
}

export function isAppRuntime() {
    return typeof window !== "undefined" && appRuntimeFrom(window.location);
}

/** Where a signed-out device goes: the app's computers page, or the companion's sign-in page. */
export function signInPath() {
    return isAppRuntime() ? "/app/computers" : "/companion";
}

export function remoteEngineBase(location?: RuntimeLocation) {
    const current = location ?? (typeof window !== "undefined" ? window.location : undefined);
    return current && remoteRuntimeFrom(current) ? `${current.origin}/engine` : undefined;
}
