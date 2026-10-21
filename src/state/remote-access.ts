import { shellEvents, shellInvoke } from "../shell";
import { signInPath } from "../runtime";
import { createSignal } from "solid-js";
import { t } from "./i18n";

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
    passwordUsername?: string;
    certificateFingerprint: string;
    error?: string;
};

/** A one-time code the Drift app scans to link this PC. */
export type RemotePairing = { uri: string; qr: string; expiresAt: number };

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

/** Whether a pairing code also reaches the PC through the tunnel, away from its network. */
export function pairingReachesAnywhere(pairing: RemotePairing) {
    return new URL(pairing.uri).searchParams.has("h");
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

/** A fresh pairing code for the Drift app; it replaces any shown before. */
export async function showRemotePairing() {
    const invoke = shellInvoke();
    if (!invoke) throw new Error(t("drift.remote.pairRequiresDesktop"));
    return invoke<RemotePairing>("remote_access_pairing");
}

/** The app device that signed in with the code shown at `since`, once it has. */
export function pairedDevice(status: RemoteAccessStatus | null, since: number) {
    return status?.devices.find((device) => device.method === "app" && device.createdAt >= since);
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
