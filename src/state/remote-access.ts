import { shellEvents, shellInvoke } from "../shell";
import { signInPath } from "../runtime";
import { createSignal } from "solid-js";

export type RemoteDevice = {
    id: string;
    name: string;
    method: "link" | "password" | "app";
    createdAt: number;
    lastSeenAt: number;
};

export type TunnelPhase = "off" | "setting-up" | "connecting" | "online" | "reconnecting" | "failed";

export type RemoteAccessStatus = {
    enabled: boolean;
    listening: boolean;
    /** What a browser on this network opens, while browser access is on. */
    lanUrl?: string;
    browser: boolean;
    tunnelEnabled: boolean;
    tunnel: { phase: TunnelPhase; url?: string; error?: string };
    devices: RemoteDevice[];
    /** Where a device on this network connects when it does not find this computer nearby. */
    addresses: string[];
    passwordUsername?: string;
    certificateFingerprint: string;
    error?: string;
};

/** A device asking to connect, shown with the code its own screen shows. */
export type ConnectRequest = { id: string; name: string; code: string; address: string; expiresAt: number };

export function remoteStatusTone(status: RemoteAccessStatus | null) {
    if (!status) return "idle";
    if (status.error) return "error";
    return status.enabled && status.listening ? "online" : "offline";
}

export function nextRemoteAccessEnabled(status: RemoteAccessStatus | null) {
    return !status?.enabled;
}

/** The tunnel's address while it carries traffic; none while it is setting up or down. */
export function tunnelUrl(status: RemoteAccessStatus | null) {
    const tunnel = status?.tunnel;
    return tunnel?.phase === "online" || tunnel?.phase === "reconnecting" ? tunnel.url : undefined;
}

const relativeUnits = [
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
] as const;

export function lastSeenLabel(at: number, now = Date.now()) {
    const seconds = Math.round((at - now) / 1000);
    const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    const unit = relativeUnits.find(([, size]) => Math.abs(seconds) >= size);
    return unit ? format.format(Math.round(seconds / unit[1]), unit[0]) : format.format(0, "minute");
}

const [status, setStatus] = createSignal<RemoteAccessStatus | null>(null);
const [busy, setBusy] = createSignal(false);
const [error, setError] = createSignal("");
const [session, setSession] = createSignal<RemoteDevice | null>(null);

export { status as remoteAccessStatus, busy as remoteAccessBusy, error as remoteAccessError, session as remoteSession };

const message = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

async function call(command: string, args?: Record<string, unknown>) {
    const invoke = shellInvoke();
    if (!invoke) return;
    setBusy(true);
    setError("");
    try {
        setStatus(await invoke<RemoteAccessStatus>(command, args));
    } catch (cause) {
        setError(message(cause));
    } finally {
        setBusy(false);
    }
}

export function refreshRemoteAccess() {
    return call("remote_access_status");
}

export function setRemoteAccess(enabled: boolean) {
    return call(enabled ? "remote_access_enable" : "remote_access_disable");
}

/** The browser version: its sign-in page, password sign-in, and every browser's session. */
export function setRemoteBrowser(enabled: boolean) {
    return call("remote_access_set_browser", { enabled });
}

/** Reaching this PC through OpenTunnel; setting it up goes on in the background. */
export function setRemoteTunnel(enabled: boolean) {
    return call("remote_access_set_tunnel", { enabled });
}

/** The devices asking to connect now, oldest first; none outside this PC's own window. */
export async function connectRequests() {
    return (await shellInvoke()?.<ConnectRequest[]>("remote_access_requests")) ?? [];
}

export async function answerConnectRequest(id: string, accept: boolean) {
    await shellInvoke()?.("remote_access_answer", { id, accept });
}

/** Calls `changed` whenever a device starts or stops asking to connect. */
export function listenConnectRequests(changed: () => void) {
    return shellEvents()?.listen("remote-access-requests", changed);
}

export function revokeRemoteDevice(id?: string) {
    return call("remote_access_revoke", id ? { id } : {});
}

export function setRemotePassword(credentials: { username: string; password: string } | null) {
    return call("remote_access_set_password", credentials ?? {});
}

/** Keeps desktop status current as the tunnel comes up and devices link or sign out. */
export function listenRemoteAccess() {
    return shellEvents()?.listen("remote-access-changed", () => void refreshRemoteAccess());
}

export async function loadRemoteSession() {
    const response = await fetch("/auth/me", { credentials: "same-origin", cache: "no-store" });
    setSession(response.ok ? ((await response.json()) as RemoteDevice) : null);
}

export async function signOutRemoteDevice() {
    await fetch("/auth/logout", { method: "POST", credentials: "same-origin" });
    window.location.replace(signInPath());
}
