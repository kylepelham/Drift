import { isRemoteRuntime } from "../runtime";
import { backendInvoke } from "../backend";
import { shellEvents } from "../shell";
import { t } from "./i18n";

/** Theme names a snapshot may carry: the bases, and the names earlier versions saved. */
const themeNameList = [
    "drift-charcoal",
    "drift-black",
    "drift-dim",
    "drift-mocha",
    "drift-midnight",
    "drift-forest",
    "drift-plum",
    "drift-light",
    "drift-paper",
    "drift-custom",
    "drift-dark",
    "drift-graphite",
    "drift-slate",
    "drift-aubergine",
] as const;
type MirrorThemeName = (typeof themeNameList)[number];

export type MirrorTheme = {
    name: MirrorThemeName;
    /** A named accent or `#rrggbb`; empty for the base's own. */
    accent: string;
    corners: "" | "sharp" | "round";
    custom: { background: string; surface: string; text: string; accent: string };
    uiFont: string;
    codeFont: string;
    customCss: string;
};

export type MirrorSelection = { workspaceId: string | null; sessionId: string | null };
export type UiMirrorSnapshot = {
    schema: 1;
    revision: number;
    theme: MirrorTheme;
    selection: MirrorSelection;
    workspaceOrder: string[];
};

type MirrorApplier = {
    theme: (theme: MirrorTheme) => void;
    selection: (selection: MirrorSelection) => void;
    order: (ids: string[]) => void;
};

type MirrorPatch = { theme?: MirrorTheme; selection?: MirrorSelection; workspaceOrder?: string[] };

const defaults: MirrorTheme = {
    name: "drift-charcoal",
    accent: "",
    corners: "",
    custom: { background: "#161616", surface: "#1e1e1e", text: "#ececec", accent: "#a78bfa" },
    uiFont: "",
    codeFont: "",
    customCss: "",
};
const themeNames = new Set<string>(themeNameList);
const hexColor = /^#[\da-f]{6}$/i;

/** A named accent or hex colour as the shell accepts it; anything else is the base's own. */
function accentValue(value: unknown) {
    if (typeof value !== "string") return "";

    return /^[a-z]{1,16}$/.test(value) || hexColor.test(value) ? value : "";
}

function cornersValue(value: unknown): MirrorTheme["corners"] {
    return value === "sharp" || value === "round" ? value : "";
}

const clientId = globalThis.crypto?.randomUUID?.() ?? `client-${Date.now()}-${Math.random()}`;
let current: UiMirrorSnapshot | undefined;
let applier: MirrorApplier | undefined;
let queued: MirrorPatch | undefined;
let retry:
    | {
          patch: MirrorPatch;
          mutation: MirrorPatch & { clientId: string; mutationId: string };
      }
    | undefined;
let publishing = false;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let liveStarted = false;
let liveError = "";
const liveErrorListeners = new Set<(error: string) => void>();

function stored<T>(key: string, fallback: T): T {
    try {
        const value = localStorage.getItem(key);
        return value ? (JSON.parse(value) as T) : fallback;
    } catch {
        return fallback;
    }
}

function boundedString(value: unknown, fallback: string, max: number) {
    return typeof value === "string" ? [...value].slice(0, max).join("") : fallback;
}

function identifier(value: unknown) {
    return typeof value === "string" &&
        [...value].length > 0 &&
        [...value].length <= 256 &&
        ![...value].some((char) => /\p{Cc}/u.test(char))
        ? value
        : null;
}

function customTheme(value: unknown): MirrorTheme["custom"] {
    const source = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
    return {
        background:
            typeof source.background === "string" && hexColor.test(source.background)
                ? source.background
                : defaults.custom.background,
        surface:
            typeof source.surface === "string" && hexColor.test(source.surface)
                ? source.surface
                : defaults.custom.surface,
        text: typeof source.text === "string" && hexColor.test(source.text) ? source.text : defaults.custom.text,
        accent:
            typeof source.accent === "string" && hexColor.test(source.accent) ? source.accent : defaults.custom.accent,
    };
}

export function localMirrorSnapshot(): UiMirrorSnapshot {
    const name = stored<unknown>("drift.theme", defaults.name);
    const workspaceId = identifier(stored<unknown>("drift.workspace", null));
    const sessionId = workspaceId ? identifier(stored<unknown>("drift.session", null)) : null;
    const order = stored<unknown>("drift.workspace.order", []);
    const workspaceOrder = Array.isArray(order)
        ? [...new Set(order.map(identifier).filter((id): id is string => !!id))].slice(0, 500)
        : [];
    return {
        schema: 1,
        revision: 0,
        theme: {
            name:
                typeof name === "string" && themeNames.has(name as MirrorThemeName)
                    ? (name as MirrorThemeName)
                    : defaults.name,
            accent: accentValue(stored<unknown>("drift.theme.accent", "")),
            corners: cornersValue(stored<unknown>("drift.theme.corners", "")),
            custom: customTheme(stored<unknown>("drift.theme.custom", defaults.custom)),
            uiFont: boundedString(stored<unknown>("drift.theme.uiFont", ""), "", 256),
            codeFont: boundedString(stored<unknown>("drift.theme.codeFont", ""), "", 256),
            customCss: boundedString(stored<unknown>("drift.theme.customCss", ""), "", 20_000),
        },
        selection: { workspaceId, sessionId },
        workspaceOrder,
    };
}

function cache(snapshot: UiMirrorSnapshot) {
    const values: [string, unknown][] = [
        ["drift.theme", snapshot.theme.name],
        ["drift.theme.accent", snapshot.theme.accent ?? ""],
        ["drift.theme.corners", snapshot.theme.corners ?? ""],
        ["drift.theme.custom", snapshot.theme.custom],
        ["drift.theme.uiFont", snapshot.theme.uiFont],
        ["drift.theme.codeFont", snapshot.theme.codeFont],
        ["drift.theme.customCss", snapshot.theme.customCss],
        ["drift.workspace", snapshot.selection.workspaceId],
        ["drift.session", snapshot.selection.sessionId],
        ["drift.workspace.order", snapshot.workspaceOrder],
    ];
    for (const [key, value] of values) cacheValue(key, value);
}

function cacheValue(key: string, value: unknown) {
    try {
        localStorage.setItem(key, JSON.stringify(value));
    } catch {}
}

export function acceptMirrorSnapshot(snapshot: UiMirrorSnapshot, force = false) {
    if (!force && current && !shouldAcceptRevision(current.revision, snapshot.revision)) return false;
    current = snapshot;
    cache(snapshot);
    applier?.theme(snapshot.theme);
    applier?.order(snapshot.workspaceOrder);
    applier?.selection(snapshot.selection);
    return true;
}

export function shouldAcceptRevision(currentRevision: number, incomingRevision: number) {
    return incomingRevision > currentRevision;
}

export function mirrorBootstrapCommand(remote: boolean) {
    return remote ? "ui_state_snapshot" : "ui_state_initialize";
}

export function shellTimeoutBootstrapCommand(remote: boolean) {
    return remote ? "shell_timeout_snapshot" : "shell_timeout_initialize";
}

export function registerMirrorApplier(next: MirrorApplier) {
    applier = next;
    if (current) {
        next.theme(current.theme);
        next.order(current.workspaceOrder);
        next.selection(current.selection);
    }
}

export async function bootstrapMirror() {
    const invoke = backendInvoke();
    const remote = isRemoteRuntime();
    // The browser dev loop has no host to share state with: it keeps this window's own.
    if (!invoke && !remote) return void acceptMirrorSnapshot(localMirrorSnapshot(), true);
    if (!invoke) throw new Error(t("drift.host.bridgeUnavailable"));
    const snapshot = remote
        ? await invoke<UiMirrorSnapshot>(mirrorBootstrapCommand(true))
        : await invoke<UiMirrorSnapshot>(mirrorBootstrapCommand(false), { snapshot: localMirrorSnapshot() });
    acceptMirrorSnapshot(snapshot, true);

    const savedTimeout = stored<unknown>("drift.shell.timeout", null);
    const localTimeout =
        typeof savedTimeout === "number" &&
        Number.isInteger(savedTimeout) &&
        savedTimeout >= 60_000 &&
        savedTimeout <= 86_400_000
            ? savedTimeout
            : null;
    const policy = remote
        ? await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(true))
        : await invoke<{ timeoutMs: number | null }>(shellTimeoutBootstrapCommand(false), {
              policy: { timeoutMs: localTimeout },
          });
    cacheValue("drift.shell.timeout", policy.timeoutMs);
}

export function publishMirrorTheme(theme: MirrorTheme) {
    if (current) current = { ...current, theme };
    queueMutation({ theme });
}

export function publishMirrorSelection(selection: MirrorSelection) {
    if (current) current = { ...current, selection };
    queueMutation({ selection });
}

export function publishMirrorWorkspaceOrder(ids: string[]) {
    if (current && current.workspaceOrder.join("\u0000") === ids.join("\u0000")) return;
    if (current) current = { ...current, workspaceOrder: ids };
    queueMutation({ workspaceOrder: ids });
}

function queueMutation(patch: MirrorPatch) {
    if (!current) return;
    queued = { ...queued, ...patch };
    if (!publishing) void flushMutations();
}

async function flushMutations() {
    const invoke = backendInvoke();
    if (!invoke) return;
    if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = undefined;
    }
    if (retry && queued) {
        queued = { ...retry.patch, ...queued };
        retry = undefined;
    }
    publishing = true;
    let failed = false;
    try {
        while (retry || queued) {
            if (!retry) {
                prepareMutation();
            }
            try {
                const snapshot = await invoke<UiMirrorSnapshot>("ui_state_update", { mutation: retry!.mutation });
                retry = undefined;
                acceptMirrorSnapshot(snapshot);
                setLiveError("");
            } catch (cause) {
                const message = cause instanceof Error ? cause.message : String(cause);
                if (message === "selected workspace does not exist") {
                    retry = undefined;
                    setLiveError("");
                    continue;
                }
                failed = true;
                setLiveError(message);
                break;
            }
        }
    } finally {
        publishing = false;
        if (failed) {
            retryTimer = setTimeout(() => {
                retryTimer = undefined;
                void flushMutations();
            }, 1500);
        } else if ((retry || queued) && !retryTimer) {
            void flushMutations();
        }
    }
}

function prepareMutation() {
    const patch = queued!;
    queued = undefined;
    retry = {
        patch,
        mutation: {
            clientId,
            mutationId: globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`,
            ...patch,
        },
    };
}

export function startMirrorEvents() {
    if (liveStarted) return;
    liveStarted = true;
    if (!isRemoteRuntime()) {
        void shellEvents()?.listen<UiMirrorSnapshot>("ui-state-changed", (event) =>
            acceptMirrorSnapshot(event.payload),
        );
        return;
    }
    connectRemoteEvents();
}

function connectRemoteEvents() {
    const source = new EventSource("/api/ui-state/events", { withCredentials: true });
    source.onopen = () => setLiveError("");
    source.onmessage = (event) => {
        try {
            acceptMirrorSnapshot(JSON.parse(event.data) as UiMirrorSnapshot);
            setLiveError("");
        } catch {
            setLiveError(t("drift.host.invalidMirror"));
        }
    };
    source.onerror = () => {
        source.close();
        setLiveError(t("drift.host.reconnecting"));
        setTimeout(async () => {
            try {
                const snapshot = await backendInvoke()!<UiMirrorSnapshot>("ui_state_snapshot");
                acceptMirrorSnapshot(snapshot);
                connectRemoteEvents();
            } catch (cause) {
                setLiveError(cause instanceof Error ? cause.message : String(cause));
                setTimeout(connectRemoteEvents, 3000);
            }
        }, 1000);
    };
}

function setLiveError(error: string) {
    liveError = error;
    for (const listener of liveErrorListeners) listener(error);
}

export function listenMirrorLiveError(listener: (error: string) => void) {
    liveErrorListeners.add(listener);
    listener(liveError);
    return () => liveErrorListeners.delete(listener);
}

export function currentMirrorSnapshot() {
    return current;
}
