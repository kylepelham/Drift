import { configFrom, draftFrom } from "../state/plugin-settings";
import { PluginSettingsForm } from "./settings-plugin-form";
import { fieldText } from "../state/plugin-registry";
import { createSignal, For, Show } from "solid-js";
import { LogoTile } from "./logo-tile";
import { IconArrowUp } from "./icons";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { ConfigField, RegistryPlugin } from "../state/plugin-registry";
import type { PluginInfo } from "../engine/native/client";

/** Shows stored plugin config values, with defaults for fields that have not been set. */
export function ConfigFields(props: {
    fields: ConfigField[];
    typed: Record<string, string>;
    values: Record<string, unknown>;
    onTyped: (typed: Record<string, string>) => void;
}) {
    const value = (field: ConfigField) => {
        const stored = () => (field.key in props.values ? props.values[field.key] : field.default);

        return props.typed[field.key] ?? fieldText(field, stored());
    };
    const setField = (field: ConfigField, text: string) => props.onTyped({ ...props.typed, [field.key]: text });

    return (
        <div class="space-y-3 rounded-lg border border-edge bg-surface p-3">
            <div class="text-xs text-ink-muted">{t("drift.plugins.configNote")}</div>
            <For each={props.fields}>
                {(field) => (
                    <Show
                        when={field.type !== "boolean"}
                        fallback={
                            <div class="flex items-center justify-between gap-3">
                                <div class="min-w-0">
                                    <div class="text-xs font-medium text-ink">{field.label}</div>
                                    <Show when={field.description}>
                                        {(text) => (
                                            <div class="text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                                                {text()}
                                            </div>
                                        )}
                                    </Show>
                                </div>
                                <Toggle
                                    label={field.label}
                                    checked={value(field) === "true"}
                                    onChange={() => setField(field, value(field) === "true" ? "false" : "true")}
                                />
                            </div>
                        }
                    >
                        <label class="block space-y-1">
                            <div class="flex items-baseline gap-2 text-xs">
                                <span class="font-medium text-ink">{field.label}</span>
                                <span class="text-ink-faint">{t(`drift.plugins.fieldType.${field.type}`)}</span>
                            </div>
                            <Show
                                when={field.type === "json"}
                                fallback={
                                    <input
                                        type="text"
                                        autocomplete="off"
                                        spellcheck={false}
                                        class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                                        value={value(field)}
                                        onInput={(event) => setField(field, event.currentTarget.value)}
                                    />
                                }
                            >
                                <textarea
                                    spellcheck={false}
                                    class="h-32 w-full resize-y rounded-md border border-edge bg-raised/45 p-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                                    value={value(field)}
                                    onInput={(event) => setField(field, event.currentTarget.value)}
                                />
                            </Show>
                            <Show when={field.description}>
                                {(text) => (
                                    <div class="text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                                        {text()}
                                    </div>
                                )}
                            </Show>
                        </label>
                    </Show>
                )}
            </For>
        </div>
    );
}

/** Edits an installed plugin's settings with the form it declares; a plugin that declares none has no editor. */
export function EditSheet(props: {
    plugin: PluginInfo;
    registry?: RegistryPlugin;
    busy: boolean;
    onBack: () => void;
    onSave: (config: unknown) => Promise<void>;
}) {
    const stored = () =>
        props.plugin.config && typeof props.plugin.config === "object"
            ? (props.plugin.config as Record<string, unknown>)
            : {};
    const declared = () => props.plugin.settings ?? [];
    const [draft, setDraft] = createSignal(draftFrom(declared(), stored()));

    return (
        <div class="space-y-4">
            <button
                class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink"
                onClick={() => props.onBack()}
            >
                <IconArrowUp class="size-3.5 -rotate-90" />
                {t("common.back")}
            </button>
            <div class="flex items-start gap-3">
                <LogoTile image={props.registry?.image} title={props.plugin.name} large />
                <div class="min-w-0 flex-1">
                    <div class="text-base font-semibold text-ink">{props.plugin.name}</div>
                    <div class="font-mono text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint">
                        {props.plugin.path}
                    </div>
                    <Show when={props.registry?.description}>
                        {(text) => <div class="mt-2 text-sm text-ink-muted">{text()}</div>}
                    </Show>
                </div>
            </div>
            <PluginSettingsForm settings={declared()} draft={draft()} onDraft={setDraft} />
            <div class="flex items-center justify-end gap-2">
                <button
                    class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink"
                    onClick={() => props.onBack()}
                >
                    {t("common.cancel")}
                </button>
                <button
                    class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={props.busy}
                    onClick={() => void props.onSave(configFrom(declared(), draft(), stored()))}
                >
                    {t(props.busy ? "drift.plugins.saving" : "common.save")}
                </button>
            </div>
        </div>
    );
}
