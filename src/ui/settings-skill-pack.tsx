import { IconArrowUp, IconArrowUpRight, IconCheck, IconPlus } from "./icons";
import { createSignal, For, Show } from "solid-js";
import { openExternal } from "../shell";
import { LogoTile } from "./logo-tile";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { RegistryPlugin } from "../state/plugin-registry";

export const skillsFolder = "~/.config/drift/skills";

/** Shows a pack's skills before installation, with all skills initially selected. */
export function PackSheet(props: {
    pack: RegistryPlugin;
    installed: boolean;
    disabled: boolean;
    busy: boolean;
    onBack: () => void;
    onInstall: (skills: string[]) => Promise<void>;
}) {
    const all = () => props.pack.skills ?? [];
    const [excluded, setExcluded] = createSignal(new Set<string>());
    const chosen = () =>
        all()
            .filter((skill) => !excluded().has(skill.name))
            .map((skill) => skill.name);
    const everyOn = () => excluded().size === 0;

    const toggleSkill = (name: string) =>
        setExcluded((current) => {
            const next = new Set(current);
            if (next.has(name)) next.delete(name);
            else next.add(name);

            return next;
        });

    return (
        <div class="space-y-4">
            <button
                class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink"
                onClick={() => props.onBack()}
            >
                <IconArrowUp class="size-3.5 -rotate-90" />
                {t("drift.mcp.registry.back")}
            </button>
            <div class="flex items-start gap-3">
                <LogoTile image={props.pack.image} title={props.pack.name} large />
                <div class="min-w-0 flex-1">
                    <div class="text-base font-semibold text-ink">{props.pack.name}</div>
                    <div class="text-[0.7rem] text-ink-faint">
                        {props.pack.author} · {props.pack.version}
                    </div>
                    <div class="mt-2 text-sm text-ink-muted">{props.pack.description}</div>
                    <div class="mt-2 flex flex-wrap items-center gap-3 text-xs">
                        <button
                            class="flex items-center gap-0.5 text-accent hover:underline"
                            onClick={() => openExternal(props.pack.source)}
                        >
                            {t("drift.plugins.source")}
                            <IconArrowUpRight class="size-3" />
                        </button>
                        <span class="font-mono text-ink-faint">
                            {skillsFolder}/{props.pack.id}/
                        </span>
                    </div>
                </div>
            </div>
            <div class="rounded-lg border border-edge bg-surface">
                <Show when={all().length > 1}>
                    <div class="flex items-center justify-between gap-3 border-b border-edge/70 px-3 py-2">
                        <div class="text-xs text-ink-muted">
                            {t("drift.skills.choose", { on: chosen().length, count: all().length })}
                        </div>
                        <button
                            class="text-xs text-accent hover:underline"
                            onClick={() =>
                                setExcluded(everyOn() ? new Set(all().map((skill) => skill.name)) : new Set())
                            }
                        >
                            {t(everyOn() ? "drift.skills.none" : "drift.skills.all")}
                        </button>
                    </div>
                </Show>
                <For each={all()}>
                    {(skill) => (
                        <div
                            class="flex min-h-11 cursor-pointer items-center gap-3 border-b border-edge/70 px-3 py-2 last:border-b-0 hover:bg-raised/40"
                            classList={{ "opacity-60": excluded().has(skill.name) }}
                            onClick={() => toggleSkill(skill.name)}
                        >
                            <div class="min-w-0 flex-1">
                                <div class="truncate text-[0.82rem] font-medium text-ink">{skill.name}</div>
                                <Show when={skill.description}>
                                    <div class="truncate text-xs text-ink-faint" title={skill.description}>
                                        {skill.description}
                                    </div>
                                </Show>
                            </div>
                            <Show when={all().length > 1}>
                                <Toggle
                                    label={skill.name}
                                    checked={!excluded().has(skill.name)}
                                    onChange={() => toggleSkill(skill.name)}
                                />
                            </Show>
                        </div>
                    )}
                </For>
            </div>
            <div class="text-[0.7rem] text-ink-faint">
                {t("drift.plugins.packNote", { folder: `${skillsFolder}/${props.pack.id}` })}
            </div>
            <div class="flex items-center justify-end gap-2">
                <button
                    class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink"
                    onClick={() => props.onBack()}
                >
                    {t("common.cancel")}
                </button>
                <button
                    class="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={props.disabled || props.busy || !chosen().length}
                    onClick={() => void props.onInstall(chosen())}
                >
                    {props.installed ? <IconCheck class="size-3.5" /> : <IconPlus class="size-3.5" />}
                    {t(packInstallLabel(props.busy, props.installed))}
                </button>
            </div>
        </div>
    );
}

function packInstallLabel(busy: boolean, installed: boolean) {
    if (busy) return "drift.plugins.installing";
    if (installed) return "drift.skills.reinstall";

    return "drift.plugins.install";
}
