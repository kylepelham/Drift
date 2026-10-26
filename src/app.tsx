import { activeWorkspace, initWorkspaces, purgeAll, workspaces } from "./state/workspaces";
import { createEffect, createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { nudgesSincePrompt, orchestratorNotice } from "./state/orchestrator";
import { closeMobileDrawer, mobileDrawerOpen } from "./state/navigation";
import { AttentionNotifier, NoticeHost } from "./ui/notifications";
import { messageText, createRunWatcher } from "./engine/store";
import { ToolContextMenuHost } from "./ui/tool-context-menu";
import { browserPaneShowing } from "./state/browser-pane";
import { syncDictationConsent } from "./voice/dictation";
import { initDevtoolsShortcut } from "./state/devtools";
import { ImportSummaryHost } from "./ui/import-summary";
import { bindShellTimeoutPolicy } from "./state/prefs";
import { listenMirrorLiveError } from "./state/mirror";
import { setPluginGrammars } from "./syntax-grammars";
import { EngineProvider, useEngine } from "./engine";
import { forwardWheelToChat } from "./ui/chat-wheel";
import { selectedSession } from "./state/selection";
import { FilePreviewHost } from "./ui/file-preview";
import { ReviewPanelHost } from "./ui/review-panel";
import { BrowserPaneHost } from "./ui/browser-pane";
import { messageProblem } from "./engine/messages";
import { bindCodePreferences } from "./state/code";
import { ChatBackdrop } from "./ui/chat-backdrop";
import { bindTextSizes } from "./state/text-size";
import { hiddenParent } from "./engine/sessions";
import { initKeybinds } from "./state/keybinds";
import { bindLanguage } from "./state/language";
import { rightPanelOpen } from "./state/panels";
import { bindPathCopy } from "./ui/path-copy";
import { SettingsHost } from "./ui/settings";
import { StartupSplash } from "./ui/startup";
import { McpServersModal } from "./ui/mcp";
import { PaletteHost } from "./ui/palette";
import { bindTheme } from "./state/theme";
import { Composer } from "./ui/composer";
import { ChatHeader } from "./ui/header";
import { Lightbox } from "./ui/lightbox";
import { initZoom } from "./state/zoom";
import { DebugPanel } from "./ui/debug";
import { PluginHost } from "./plugins";
import { Sidebar } from "./ui/sidebar";
import { Chat } from "./ui/chat";
import { t } from "./state/i18n";

export function App() {
    bindTheme();
    bindTextSizes();
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
            <PathCopyBinding />
            <div class="app-shell flex h-full flex-col bg-bg text-ink">
                <div class="flex min-h-0 flex-1">
                    <Show when={mobileDrawerOpen()}>
                        <button
                            aria-label={t("common.close")}
                            class="mobile-sidebar-backdrop fixed inset-0 z-30 bg-black/55"
                            onClick={() => closeMobileDrawer()}
                        />
                    </Show>
                    <Sidebar />
                    <main inert={mobileDrawerOpen()} class="flex min-h-0 min-w-0 flex-1 overflow-hidden">
                        <div
                            class="chat-pane relative isolate flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
                            classList={{
                                "chat-pane-covered": (rightPanelOpen() || browserPaneShowing()) && !!selectedSession(),
                            }}
                        >
                            <ChatBackdrop />
                            <div class="relative flex min-h-0 flex-1 flex-col">
                                <ChatHeader />
                                <Chat />
                            </div>
                            <div
                                class="composer-dock relative shrink-0 px-4 pb-4"
                                onWheel={(event) => forwardWheelToChat(event, event.currentTarget)}
                            >
                                <Composer />
                            </div>
                        </div>
                        <BrowserPaneHost />
                        <DebugPanel />
                        <ReviewPanelHost />
                    </main>
                </div>
                <Lightbox />
                <FilePreviewHost />
                <McpServersModal />
                <SettingsHost />
                <PaletteHost />
                <ToolContextMenuHost />
                <ImportSummaryHost />
                <NoticeHost />
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
            <div class="mirror-notice fixed right-3 bottom-3 z-20 max-w-sm rounded-md border border-danger/35 bg-surface px-3 py-2 text-xs text-danger shadow-lg">
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

/** Double-clicking a path copies it with this platform's separators, and says so. */
function PathCopyBinding() {
    const engine = useEngine();
    onCleanup(
        bindPathCopy(
            (path) =>
                engine.actions.notice({
                    message: t("drift.path.copied", { path }),
                    variant: "success",
                    duration: 2000,
                }),
            (path) => engine.actions.notice({ message: t("drift.path.copyFailed", { path }), variant: "error" }),
        ),
    );
    return null;
}

/** The engine drives orchestrator turns; this only says how one ended. */
function OrchestratorBinding() {
    const engine = useEngine();
    // Reporting after the update settles, as before: the notice it may post is a write of its own.
    createRunWatcher(
        () => engine.state,
        (id, was) => queueMicrotask(() => report(id, was)),
    );

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
// Other workspaces poll less often: a sweep may boot an engine for a directory not loaded.
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
    createEffect(() => setPluginGrammars(engine.state.grammars));
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
            // A failed engine deletion retries on the next reconnect or hourly tick, not the next day.
            if (!complete) lastPurge = 0;
        });
    }
}
