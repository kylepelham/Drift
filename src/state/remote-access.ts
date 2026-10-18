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
export type PendingLink = { name: string; address: string; requestedAt: number };

export type RemoteAccessStatus = {
    enabled: boolean;
    listening: boolean;
    port: number;
    discoveryPort: number;
    listeningAddress?: string;
    urls: string[];
    addressQr?: string;
    devices: RemoteDevice[];
    pendingLinks: PendingLink[];
    passwordUsername?: string;
    certificateFingerprint: string;
    /** The fingerprint as six words, which the Drift app shows while it links by code. */
    certificateWords: string[];
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

/** A typed code in any case or grouping; only the 8 code characters are compared. */
export function normalizeLinkCode(input: string) {
    return input.replace(/[^a-z0-9]/gi, "").toUpperCase();
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

/** Approves the device showing `code`; resolves to its name. */
export async function linkRemoteDevice(code: string) {
    const invoke = shellInvoke();
    if (!invoke) throw new Error(t("drift.remote.linkRequiresDesktop"));
    const name = await invoke<string>("remote_access_link", { code: normalizeLinkCode(code) });
    void refreshRemoteAccess();
    return name;
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

/** Keeps desktop status current as devices ask to link, finish linking, or sign out. */
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
