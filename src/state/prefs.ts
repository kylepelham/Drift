import { backendInvoke } from "../backend";
import { shellEvents } from "../shell";
import { persisted } from "./persist";

import type { ModelRef } from "../engine/store";

export const attentionKinds = ["agent", "permission", "error"] as const;
export type AttentionKind = (typeof attentionKinds)[number];
export type AlertSound = "none" | "custom" | string;
export type CustomSound = { name: string; dataUrl: string };
export const shellTimeoutPresets = [60_000, 300_000, 900_000, 1_800_000] as const;
export const shellTimeoutMinMs = 60_000;
export const shellTimeoutMaxMs = 86_400_000;
export const responseAnimationSpeedMin = 60;
export const responseAnimationSpeedMax = 600;
export const responseAnimationSpeedDefault = 144;

export function normalizeResponseAnimationSpeed(value: unknown) {
    if (typeof value !== "number" || !Number.isFinite(value)) return responseAnimationSpeedDefault;
    return Math.min(responseAnimationSpeedMax, Math.max(responseAnimationSpeedMin, Math.round(value)));
}

export function normalizeShellTimeout(value: unknown): number | null {
    if (value === null) return null;
    if (typeof value !== "number" || !Number.isSafeInteger(value)) return null;
    return value >= shellTimeoutMinMs && value <= shellTimeoutMaxMs ? value : null;
}

export function notificationDefaults(enabled: boolean) {
    return Object.fromEntries(attentionKinds.map((kind) => [kind, enabled])) as Record<AttentionKind, boolean>;
}

export function soundDefaults() {
    return Object.fromEntries(attentionKinds.map((kind) => [kind, "none"])) as Record<AttentionKind, AlertSound>;
}

export const [modelPref, setModelPref] = persisted<ModelRef | null>("drift.model", null);
export const [agentPref, setAgentPref] = persisted<string>("drift.agent", "build");
export const [variantPref, setVariantPref] = persisted<string | null>("drift.variant", null);
export const [hiddenModelIds, setHiddenModelIds] = persisted<string[]>("drift.models.hidden", []);
export const [shownModelIds, setShownModelIds] = persisted<string[]>("drift.models.shown", []);
export const [modelProviderOrder, setModelProviderOrder] = persisted<string[]>("drift.models.providerOrder", []);
export const [showReasoning, setShowReasoning] = persisted<boolean>("drift.reasoning", false);
export const [toolErrorsExpanded, setToolErrorsExpanded] = persisted<boolean>("drift.toolErrors.expanded", false);
export const [animateResponses, setAnimateResponses] = persisted<boolean>("drift.responses.animate", false);
export const [responseAnimationSpeed, setResponseAnimationSpeed] = persisted<number>(
    "drift.responses.speed",
    responseAnimationSpeedDefault,
    normalizeResponseAnimationSpeed,
);
export const [shellTimeoutMs, setShellTimeoutValue] = persisted<number | null>(
    "drift.shell.timeout",
    null,
    normalizeShellTimeout,
);
let shellTimeoutErrorValue = "";
const timeoutErrorListeners = new Set<(error: string) => void>();

export function listenShellTimeoutError(listener: (error: string) => void) {
    timeoutErrorListeners.add(listener);
    listener(shellTimeoutErrorValue);
    return () => timeoutErrorListeners.delete(listener);
}

export function reportShellTimeoutError(error: string) {
    shellTimeoutErrorValue = error;
    for (const listener of timeoutErrorListeners) listener(error);
}

export async function setShellTimeoutMs(value: number | null) {
    const timeoutMs = normalizeShellTimeout(value);
    if (value !== null && timeoutMs === null) throw new Error("Invalid shell timeout");
    const invoke = backendInvoke();
    if (!invoke) {
        setShellTimeoutValue(timeoutMs);
        return;
    }
    const previous = shellTimeoutMs();
    setShellTimeoutValue(timeoutMs);
    try {
        const policy = await invoke<{ timeoutMs: number | null }>("shell_timeout_update", { policy: { timeoutMs } });
        setShellTimeoutValue(policy.timeoutMs);
        reportShellTimeoutError("");
    } catch (cause) {
        setShellTimeoutValue(previous);
        const message = cause instanceof Error ? cause.message : String(cause);
        reportShellTimeoutError(message);
        throw cause;
    }
}

export function bindShellTimeoutPolicy() {
    const events = shellEvents();
    if (!events) return;
    void events.listen<{ timeoutMs: number | null }>("shell-timeout-changed", (event) => {
        setShellTimeoutValue(event.payload.timeoutMs);
        reportShellTimeoutError("");
    });
}
const [legacyNotifications] = persisted<boolean>("drift.notifications", false);
export const [systemNotifications, setSystemNotifications] = persisted<Record<AttentionKind, boolean>>(
    "drift.notifications.events",
    notificationDefaults(legacyNotifications()),
);
export const [alertSounds, setAlertSounds] = persisted<Record<AttentionKind, AlertSound>>(
    "drift.notifications.sounds",
    soundDefaults(),
);
export const [customSound, setCustomSound] = persisted<CustomSound | null>("drift.notifications.customSound", null);
export const [collapseCompaction, setCollapseCompaction] = persisted<boolean>("drift.compaction.collapsible", true);
export const [compactionCollapsed, setCompactionCollapsed] = persisted<boolean>("drift.compaction.collapsed", true);
export const [autoUpdate, setAutoUpdate] = persisted<boolean>("drift.autoUpdate", true);
/** Day headings (Today, Yesterday, a weekday, a date) between a workspace's threads in the sidebar. */
export const [sidebarDayDividers, setSidebarDayDividers] = persisted<boolean>("drift.sidebar.dayDividers", false);
export type KeptAutoAccept = { all: boolean; sessions: string[] };

function keptAutoAccept(): KeptAutoAccept {
    try {
        const all = localStorage.getItem("drift.autoAccept.global") === "true";
        const listed: unknown = JSON.parse(localStorage.getItem("drift.autoAccept") ?? "[]");
        return {
            all,
            sessions: Array.isArray(listed) ? listed.filter((id): id is string => typeof id === "string") : [],
        };
    } catch {
        return { all: false, sessions: [] };
    }
}

/** Auto-accept this webview kept before the engine owned it. `hand` gives it to the engine and returns
 * what the engine did not take; only what it took is forgotten, and a `hand` that throws forgets nothing. */
export async function handOverAutoAccept(hand: (kept: KeptAutoAccept) => Promise<KeptAutoAccept>) {
    const kept = keptAutoAccept();
    if (!kept.all && !kept.sessions.length) return;
    const left = await hand(kept);
    if (!left.all) localStorage.removeItem("drift.autoAccept.global");
    if (left.sessions.length) localStorage.setItem("drift.autoAccept", JSON.stringify(left.sessions));
    else localStorage.removeItem("drift.autoAccept");
}

export function setSystemNotification(kind: AttentionKind, enabled: boolean) {
    setSystemNotifications({ ...notificationDefaults(false), ...systemNotifications(), [kind]: enabled });
}

export function setAlertSound(kind: AttentionKind, sound: AlertSound) {
    setAlertSounds({ ...soundDefaults(), ...alertSounds(), [kind]: sound });
}

/** Choices made in the composer for a session and not yet sent; an accepted send clears them. */
type SessionPrefs = { model?: ModelRef | null; agent?: string; variant?: string | null };
const [sessionPrefs, setSessionPrefs] = persisted<Record<string, SessionPrefs>>("drift.session.prefs", {});

/** What the session runs as next by the engine's account; `{}` for a session it has not reported. */
export type SessionChoice = { agent?: string; variant?: string | null; model?: ModelRef };

/** An unsent edit, else what the session runs as, else the global default. */
export function prefsFor(sessionId: string | null | undefined, saved: SessionChoice) {
    const own = (sessionId && sessionPrefs()[sessionId]) || {};

    return {
        model: own.model !== undefined ? own.model : (saved.model ?? modelPref()),
        agent: own.agent ?? saved.agent ?? agentPref(),
        variant: preferredVariant(own.variant, saved.variant),
    };
}

function preferredVariant(own: string | null | undefined, saved: string | null | undefined) {
    if (own !== undefined) return own;
    if (saved !== undefined) return saved;

    return variantPref();
}

/** The level to send: null for the model's default, `undefined` (left out, so the session keeps its own) for one this model does not offer. */
export function sendableVariant(pref: string | null | undefined, offered: readonly string[]) {
    if (pref === null) return null;
    return pref && offered.includes(pref) ? pref : undefined;
}

/** The session now runs as what was sent, so the composer goes back to showing the session's own choice. */
export function clearEdits(sessionId: string) {
    if (!sessionPrefs()[sessionId]) return;
    const next = { ...sessionPrefs() };
    delete next[sessionId];
    setSessionPrefs(next);
}

export function updatePrefs(sessionId: string | null | undefined, patch: SessionPrefs) {
    if (patch.model !== undefined) setModelPref(patch.model);
    if (patch.agent !== undefined) setAgentPref(patch.agent);
    if (patch.variant !== undefined) setVariantPref(patch.variant);
    if (sessionId) setSessionPrefs({ ...sessionPrefs(), [sessionId]: { ...sessionPrefs()[sessionId], ...patch } });
}

export function seedPrefs(sessionId: string) {
    if (sessionPrefs()[sessionId]) return;
    setSessionPrefs({
        ...sessionPrefs(),
        [sessionId]: { model: modelPref(), agent: agentPref(), variant: variantPref() },
    });
}

export function setModelVisible(id: string, visible: boolean) {
    setModelsVisible([id], visible);
}

export function setModelsVisible(ids: string[], visible: boolean) {
    const changed = new Set(ids);
    setHiddenModelIds(
        visible ? hiddenModelIds().filter((id) => !changed.has(id)) : [...new Set([...hiddenModelIds(), ...ids])],
    );
    setShownModelIds(
        visible ? [...new Set([...shownModelIds(), ...ids])] : shownModelIds().filter((id) => !changed.has(id)),
    );
}

export function modelVisible(id: string, defaultVisible: boolean) {
    if (shownModelIds().includes(id)) return true;
    if (hiddenModelIds().includes(id)) return false;
    return defaultVisible;
}

export function mergeModelProviderOrder(saved: string[], available: string[]) {
    const unique = [...new Set(available)];
    return [...saved.filter((id) => unique.includes(id)), ...unique.filter((id) => !saved.includes(id))];
}

export function orderedModelProviderIds(available: string[]) {
    return mergeModelProviderOrder(modelProviderOrder(), available);
}

export function reorderModelProviderIds(order: string[], id: string, beforeID: string | null) {
    if (!order.includes(id) || id === beforeID) return order;
    const next = order.filter((item) => item !== id);
    const target = beforeID ? next.indexOf(beforeID) : next.length;
    next.splice(target < 0 ? next.length : target, 0, id);
    return next;
}

export function moveModelProvider(id: string, beforeID: string | null, available: string[]) {
    setModelProviderOrder(reorderModelProviderIds(orderedModelProviderIds(available), id, beforeID));
}
