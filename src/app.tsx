import { activeWorkspace, initWorkspaces, purgeAll, workspaces } from "./state/workspaces";
import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { nudgesSincePrompt, orchestratorNotice } from "./state/orchestrator";
import { closeMobileDrawer, mobileDrawerOpen } from "./state/navigation";
import { AttentionNotifier, NoticeHost } from "./ui/notifications";
import { ToolContextMenuHost } from "./ui/tool-context-menu";
import { RemoteLinkNotice } from "./ui/remote-link-notice";
import { syncDictationConsent } from "./voice/dictation";
import { initDevtoolsShortcut } from "./state/devtools";
import { ImportSummaryHost } from "./ui/import-summary";
import { bindShellTimeoutPolicy } from "./state/prefs";
import { listenMirrorLiveError } from "./state/mirror";
import { EngineProvider, useEngine } from "./engine";
import { Chat, forwardWheelToChat } from "./ui/chat";
import { selectedSession } from "./state/selection";
import { FilePreviewHost } from "./ui/file-preview";
import { messageProblem } from "./engine/messages";
import { bindCodePreferences } from "./state/code";
import { hiddenParent } from "./engine/sessions";
import { initKeybinds } from "./state/keybinds";
import { bindLanguage } from "./state/language";
import { debugPanelOpen } from "./state/panels";
import { messageText } from "./engine/store";
import { SettingsHost } from "./ui/settings";
import { StartupSplash } from "./ui/startup";
import { McpServersModal } from "./ui/mcp";
import { PaletteHost } from "./ui/palette";
import { bindTheme } from "./state/theme";
import { Composer } from "./ui/composer";
import { ChatHeader } from "./ui/header";
import { Lightbox } from "./ui/lightbox";
import { Titlebar } from "./ui/titlebar";
import { initZoom } from "./state/zoom";
import { DebugPanel } from "./ui/debug";
import { PluginHost } from "./plugins";
import { Sidebar } from "./ui/sidebar";
import { t } from "./state/i18n";

export function App() {
    bindTheme();
    bindCodePreferences();
    bindLanguage();
    initKeybinds();
    initZoom();
    bindShellTimeoutPolicy();
    onCleanup(initDevtoolsShortcut());
    onMount(() => void syncDictationConsent().catch(() => undefined));
    return (
        <EngineProvider>
            <WorkspaceBinding />
            <OrchestratorBinding />
            <PluginBinding />
            <div class="app-shell flex h-full flex-col bg-bg text-ink">
                <Titlebar />
                <div class="flex min-h-0 flex-1">
                    <Show when={mobileDrawerOpen()}>
                        <button
                            aria-label={t("common.close")}
                            class="mobile-sidebar-backdrop fixed inset-0 z-30 bg-black/55"
                            onClick={() => closeMobileDrawer()}
                        />
                    </Show>
                    <Sidebar />
                    <main class="flex min-h-0 min-w-0 flex-1 overflow-hidden">
                        <div
                            class="chat-pane flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
                            classList={{ "chat-pane-covered": debugPanelOpen() && !!selectedSession() }}
                        >
                            <div class="relative flex min-h-0 flex-1 flex-col">
                                <ChatHeader />
                                <Chat />
                            </div>
                            <div
                                class="composer-dock shrink-0 px-4 pb-4"
                                onWheel={(event) => forwardWheelToChat(event, event.currentTarget)}
                            >
                                <Composer />
                            </div>
                        </div>
                        <DebugPanel />
                    </main>
                </div>
                <Lightbox />
                <FilePreviewHost />
                <McpServersModal />
                <SettingsHost />
                <PaletteHost />
                <ToolContextMenuHost />
                <ImportSummaryHost />
                <NoticeHost>
                    <RemoteLinkNotice />
                </NoticeHost>
                <MirrorConnectionNotice />
            </div>
            <StartupSplash />
        </EngineProvider>
    );
}

function MirrorConnectionNotice() {
    const [error, setError] = createSignal("");
    onMount(() => {
        const stop = listenMirrorLiveError(setError);
        onCleanup(stop);
    });
    return (
        <Show when={error()}>
            <div class="fixed right-3 bottom-3 z-20 max-w-sm rounded-md border border-danger/35 bg-surface px-3 py-2 text-xs text-danger shadow-lg">
                {error()}
            </div>
        </Show>
    );
}

function PluginBinding() {
    const engine = useEngine();
    return (
        <>
            <PluginHost engine={engine} />
            <AttentionNotifier engine={engine} />
        </>
    );
}

/** The engine drives orchestrator turns; this only says how one ended. */
function OrchestratorBinding() {
    const engine = useEngine();
    const previous = new Map<string, string>();

    createEffect(() => {
        for (const [id, status] of Object.entries(engine.state.status)) {
            const before = previous.get(id);
            previous.set(id, status.type);
            // The microtask escapes the effect's tracking scope: reading the transcript must not resubscribe it.
            if (status.type === "idle" && (before === "busy" || before === "retry"))
                queueMicrotask(() => report(id, before));
        }
        for (const id of previous.keys()) if (!engine.state.status[id]) previous.delete(id);
    });

    function report(id: string, previousStatus: string) {
        const state = engine.state;
        const entries = state.transcripts[id] ?? [];
        const last = entries.at(-1);
        const prompt = [...entries].reverse().find((entry) => entry.info.role === "user");
        const notice = orchestratorNotice({
            previousStatus,
            status: state.status[id]?.type ?? "idle",
            agent: (prompt?.info as { agent?: string } | undefined)?.agent,
            parentID: hiddenParent(state.sessions[id]),
            lastMessage: last && {
                role: last.info.role,
                completed: !!last.info.finishedAt,
                errored: !!messageProblem(last.info),
                text: messageText(last),
            },
            rounds: nudgesSincePrompt(entries as never),
        });
        if (notice) engine.actions.notice(notice);
    }

    return null;
}

const dayMs = 24 * 60 * 60 * 1000;
const purgeIntervalMs = 60 * 60 * 1000;
// The active workspace is polled on every tick; every other workspace is polled less often because
// each sweep may have to boot an engine instance for a directory that is not currently loaded.
const activePermissionPollMs = 10_000;
const allWorkspacePermissionPollMs = 60_000;
const ticksPerAllWorkspaceSweep = allWorkspacePermissionPollMs / activePermissionPollMs;

function WorkspaceBinding() {
    const engine = useEngine();
    let lastPurge = 0;
    let permissionTick = 0;
    onMount(() => {
        void initWorkspaces();
    });
    createEffect(() => engine.setDirectory(activeWorkspace()?.path ?? null));
    createEffect(() => {
        if (engine.state.connection !== "online") return;
        void engine.actions.loadAllSessions();
        // Full sweep once on connect; the timer keeps the active workspace hot afterward.
        const paths = untrack(() => workspaces().map((workspace) => workspace.path));
        void engine.actions.refreshPermissions(paths);
        purge();
    });
    const timer = setInterval(() => purge(), purgeIntervalMs);
    // Global /global/event covers live asks; this recovers asks raised while offline.
    const permissionTimer = setInterval(() => refreshPermissions(), activePermissionPollMs);
    onCleanup(() => {
        clearInterval(timer);
        clearInterval(permissionTimer);
    });
    return null;

    function refreshPermissions() {
        if (engine.state.connection !== "online") return;
        const active = activeWorkspace()?.path;
        const paths = workspaces().map((workspace) => workspace.path);
        permissionTick += 1;
        if (permissionTick % ticksPerAllWorkspaceSweep === 0) {
            void engine.actions.refreshPermissions(paths);
            return;
        }
        if (active) void engine.actions.refreshPermissions([active]);
    }

    function purge() {
        if (engine.state.connection !== "online" || Date.now() - lastPurge < dayMs) return;
        lastPurge = Date.now();
        void purgeAll(engine.actions).then((complete) => {
            // Failed engine deletions kept their tombstones; clearing the stamp retries on the next
            // reconnect or hourly tick instead of waiting out the daily interval.
            if (!complete) lastPurge = 0;
        });
    }
}
