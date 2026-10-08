import { comboFor, eventCombo, formatCombo, keybindDefs, setCombo } from "../state/keybinds";
import { createEffect, createSignal, For, onCleanup } from "solid-js";
import { t } from "../state/i18n";
import { IconX } from "./icons";

import type { KeybindAction } from "../state/keybinds";

export const keybindLabels: Record<KeybindAction, string> = {
    palette: "command.palette",
    newThread: "command.session.new",
    findInSession: "drift.shortcuts.findInSession",
    autoAccept: "drift.shortcuts.autoAccept",
    zoomIn: "drift.shortcuts.zoomIn",
    zoomOut: "drift.shortcuts.zoomOut",
    zoomReset: "drift.shortcuts.zoomReset",
};

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
        <div class="space-y-1">
            <For each={keybindDefs}>
                {(definition) => (
                    <div class="flex items-center gap-2 rounded-lg px-3 py-2 hover:bg-raised/60">
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
                            onClick={() => setCapturing(capturing() === definition.action ? null : definition.action)}
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
        </div>
    );
}

function shortcutLabel(capturing: boolean, action: KeybindAction) {
    if (capturing) return `${t("settings.shortcuts.pressKeys")}...`;

    const combo = comboFor(action);

    return combo ? formatCombo(combo) : t("settings.shortcuts.unassigned");
}
