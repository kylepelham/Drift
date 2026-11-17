import { comboFor, eventCombo, formatCombo, keybindDefs, setCombo } from "../state/keybinds";
import { createEffect, createSignal, For, onCleanup } from "solid-js";
import { SettingsGroup } from "./settings-controls";
import { t } from "../state/i18n";
import { IconX } from "./icons";

import type { KeybindAction, KeybindGroup } from "../state/keybinds";

export const keybindLabels: Record<KeybindAction, string> = {
    palette: "command.palette",
    settings: "drift.shortcuts.settings",
    showShortcuts: "drift.shortcuts.showShortcuts",
    newThread: "command.session.new",
    searchSessions: "drift.search.sessions.placeholder",
    findInSession: "drift.shortcuts.findInSession",
    focusComposer: "drift.shortcuts.focusComposer",
    focusSidebar: "drift.shortcuts.focusSidebar",
    toggleSidebar: "drift.shortcuts.toggleSidebar",
    addWorkspace: "drift.sidebar.addWorkspace",
    nextSession: "drift.shortcuts.nextSession",
    previousSession: "drift.shortcuts.previousSession",
    sessionBack: "drift.shortcuts.sessionBack",
    sessionForward: "drift.shortcuts.sessionForward",
    nextWorkspace: "drift.shortcuts.nextWorkspace",
    previousWorkspace: "drift.shortcuts.previousWorkspace",
    previousMessage: "drift.shortcuts.previousMessage",
    nextMessage: "drift.shortcuts.nextMessage",
    previousPrompt: "drift.shortcuts.previousPrompt",
    nextPrompt: "drift.shortcuts.nextPrompt",
    firstMessage: "drift.shortcuts.firstMessage",
    lastMessage: "drift.shortcuts.lastMessage",
    agentPicker: "drift.shortcuts.agentPicker",
    modelPicker: "drift.shortcuts.modelPicker",
    thinkingPicker: "drift.shortcuts.thinkingPicker",
    attachFiles: "prompt.action.attachFile",
    stopTurn: "drift.shortcuts.stopTurn",
    autoAccept: "drift.shortcuts.autoAccept",
    toggleUltra: "drift.shortcuts.toggleUltra",
    renameSession: "drift.shortcuts.renameSession",
    archiveSession: "command.session.archive",
    forkSession: "drift.shortcuts.forkSession",
    toggleReview: "drift.shortcuts.toggleReview",
    toggleBrowser: "drift.shortcuts.toggleBrowser",
    mcpServers: "dialog.mcp.title",
    archivedSessions: "drift.sidebar.archived",
    cycleTheme: "command.theme.cycle",
    zoomIn: "drift.shortcuts.zoomIn",
    zoomOut: "drift.shortcuts.zoomOut",
    zoomReset: "drift.shortcuts.zoomReset",
    nextComputer: "drift.shortcuts.nextComputer",
    previousComputer: "drift.shortcuts.previousComputer",
};

const groupTitles: Record<KeybindGroup, string> = {
    general: "drift.shortcuts.group.general",
    sessions: "drift.shortcuts.group.sessions",
    messages: "drift.shortcuts.group.messages",
    composer: "drift.shortcuts.group.composer",
    session: "drift.shortcuts.group.session",
    panels: "drift.shortcuts.group.panels",
    view: "drift.shortcuts.group.view",
};

// Keys that mean something only where focus is; fixed, so shown but not rebindable.
const contextKeys: { title: string; keys: [string, string][] }[] = [
    {
        title: "drift.shortcuts.fixed.composer",
        keys: [
            ["Enter", "drift.shortcuts.key.send"],
            ["Shift+Enter", "drift.shortcuts.key.newline"],
            ["Alt+Enter", "drift.shortcuts.key.queue"],
            ["Tab / Shift+Tab", "drift.shortcuts.key.cycleAgent"],
            ["Up / Down", "drift.shortcuts.key.history"],
            ["Esc", "drift.shortcuts.key.stop"],
            ["PageUp / PageDown", "drift.shortcuts.key.scroll"],
            ["/  @", "drift.shortcuts.key.commands"],
        ],
    },
    {
        title: "drift.shortcuts.fixed.messages",
        keys: [
            ["Esc", "drift.shortcuts.key.clearHighlight"],
            ["Ctrl+C", "drift.shortcuts.key.copyMessage"],
        ],
    },
    {
        title: "drift.shortcuts.fixed.sidebar",
        keys: [
            ["Up / Down", "drift.shortcuts.key.moveRows"],
            ["Left / Right", "drift.shortcuts.key.foldWorkspace"],
            ["Enter", "drift.shortcuts.key.openSession"],
            ["F2", "drift.shortcuts.renameSession"],
            ["Delete", "command.session.archive"],
            ["Shift+F10", "drift.shortcuts.key.menu"],
        ],
    },
    {
        title: "drift.shortcuts.fixed.anywhere",
        keys: [
            ["Ctrl+1 ... Ctrl+9", "drift.shortcuts.key.sessionDigit"],
            ["Ctrl+Alt+1 ... Ctrl+Alt+9", "drift.shortcuts.key.computerDigit"],
            ["A-Z", "drift.shortcuts.key.typeAnywhere"],
            ["Up / Down, Enter, Esc", "drift.shortcuts.key.lists"],
        ],
    },
];

export function KeybindsSection() {
    const [capturing, setCapturing] = createSignal<KeybindAction | null>(null);

    createEffect(() => {
        const initialAction = capturing();
        if (!initialAction) return;

        const capture = (event: KeyboardEvent) => {
            event.preventDefault();
            event.stopPropagation();
            if (event.key === "Escape") return setCapturing(null);

            const combo = eventCombo(event);
            if (!combo) return;

            setCombo(initialAction, combo);
            setCapturing(null);
        };

        document.addEventListener("keydown", capture, true);
        onCleanup(() => document.removeEventListener("keydown", capture, true));
    });

    return (
        <div class="space-y-6">
            <For each={Object.keys(groupTitles) as KeybindGroup[]}>
                {(group) => (
                    <SettingsGroup title={t(groupTitles[group])}>
                        <For each={keybindDefs.filter((definition) => definition.group === group)}>
                            {(definition) => (
                                <div class="flex items-center gap-2 border-b border-edge/60 px-3 py-1.5 last:border-b-0">
                                    <span class="min-w-0 flex-1 truncate text-sm text-ink">
                                        {t(keybindLabels[definition.action])}
                                    </span>
                                    <button
                                        class="rounded-md border px-2.5 py-1 font-mono text-xs transition-colors"
                                        classList={{
                                            "border-accent text-accent": capturing() === definition.action,
                                            "border-edge text-ink-muted hover:border-edge-strong hover:text-ink":
                                                capturing() !== definition.action,
                                        }}
                                        onClick={() =>
                                            setCapturing(capturing() === definition.action ? null : definition.action)
                                        }
                                    >
                                        {shortcutLabel(capturing() === definition.action, definition.action)}
                                    </button>
                                    <button
                                        title={t("settings.shortcuts.unassigned")}
                                        class="flex size-6 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                                        onClick={() => setCombo(definition.action, null)}
                                    >
                                        <IconX class="size-3.5" />
                                    </button>
                                </div>
                            )}
                        </For>
                    </SettingsGroup>
                )}
            </For>
            <For each={contextKeys}>
                {(section) => (
                    <SettingsGroup title={t(section.title)}>
                        <For each={section.keys}>
                            {([keys, label]) => (
                                <div class="flex items-center gap-2 border-b border-edge/60 px-3 py-2 last:border-b-0">
                                    <span class="min-w-0 flex-1 truncate text-sm text-ink-muted">{t(label)}</span>
                                    <span class="shrink-0 font-mono text-xs text-ink-faint">{keys}</span>
                                </div>
                            )}
                        </For>
                    </SettingsGroup>
                )}
            </For>
        </div>
    );
}

function shortcutLabel(capturing: boolean, action: KeybindAction) {
    if (capturing) return `${t("settings.shortcuts.pressKeys")}...`;

    const combo = comboFor(action);

    return combo ? formatCombo(combo) : t("settings.shortcuts.unassigned");
}
