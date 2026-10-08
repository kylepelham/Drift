import { SettingsGroup } from "./settings-controls";
import { createMemo, For, Show } from "solid-js";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { components } from "../engine/native/types";

type ToolName = components["schemas"]["ToolName"];

/** Shows built-in tools and retained names that the engine no longer offers. */
export function ToolRows(props: { names: ToolName[]; chosen: string[]; onChange: (tools: string[]) => void }) {
    const builtIn = createMemo(() => {
        const known = new Set(props.names.map((tool) => tool.name));

        return [
            ...props.names.filter((tool) => !tool.server).map((tool) => tool.name),
            ...props.chosen.filter((name) => !known.has(name)),
        ];
    });
    const toggle = (name: string) => {
        const chosen = props.chosen.includes(name);
        const next = chosen ? props.chosen.filter((item) => item !== name) : [...props.chosen, name];

        props.onChange(next);
    };

    return (
        <div class="grid border-t border-edge/70 sm:grid-cols-2 sm:gap-x-8">
            <For each={builtIn()}>
                {(name) => <ToolRow label={name} on={props.chosen.includes(name)} onToggle={() => toggle(name)} />}
            </For>
        </div>
    );
}

/** Shows one switch per MCP server; permission rules can restrict individual tools. */
export function ServerRows(props: { names: ToolName[]; chosen: string[]; onChange: (tools: string[]) => void }) {
    const servers = createMemo(() => {
        const names = [...new Set(props.names.flatMap((tool) => (tool.server ? [tool.server] : [])))].sort();

        return names.map((server) => ({
            server,
            tools: props.names.filter((tool) => tool.server === server).map((tool) => tool.name),
        }));
    });
    const toggle = (tools: string[]) => {
        const all = tools.every((tool) => props.chosen.includes(tool));
        const next = all
            ? props.chosen.filter((name) => !tools.includes(name))
            : [...new Set([...props.chosen, ...tools])];

        props.onChange(next);
    };

    return (
        <Show when={servers().length}>
            <SettingsGroup title={t("drift.settings.prompts.mcpTools")}>
                <div class="grid sm:grid-cols-2 sm:gap-x-8">
                    <For each={servers()}>
                        {(entry) => {
                            const picked = () => entry.tools.filter((tool) => props.chosen.includes(tool)).length;
                            const count = () =>
                                picked() && picked() < entry.tools.length
                                    ? `${picked()} / ${entry.tools.length}`
                                    : String(entry.tools.length);

                            return (
                                <ToolRow
                                    label={entry.server}
                                    count={count()}
                                    on={picked() === entry.tools.length}
                                    onToggle={() => toggle(entry.tools)}
                                />
                            );
                        }}
                    </For>
                </div>
            </SettingsGroup>
        </Show>
    );
}

function ToolRow(props: { label: string; count?: string; on: boolean; onToggle: () => void }) {
    return (
        <div
            class="flex min-h-10 cursor-pointer items-center gap-3 border-b border-edge/70 px-1 py-1.5 hover:bg-raised/40"
            onClick={() => props.onToggle()}
        >
            <span class="min-w-0 flex-1 truncate text-[0.82rem] text-ink">{props.label}</span>
            <Show when={props.count}>
                <span class="shrink-0 text-[0.72rem] text-ink-faint">{props.count}</span>
            </Show>
            <Toggle label={props.label} checked={props.on} onChange={props.onToggle} />
        </div>
    );
}
