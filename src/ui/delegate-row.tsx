import { ToolDuration, type ToolTimingState } from "./tool-duration";
import { toolInput, type ToolPart } from "../engine/parts";
import { agentLabel, t } from "../state/i18n";
import { IconLayers } from "./icons";
import { Show } from "solid-js";

import type { DelegatedTaskStatus } from "./tool-delegation";

/** A task's own description and agent; a spawned thread keeps its tool title and names no agent. */
export function delegateLabels(part: ToolPart) {
    if (part.name !== "task") return {};
    const input = toolInput(part) as { description?: unknown; subagent_type?: unknown };
    const text = (value: unknown) => (typeof value === "string" && value.trim() ? value.trim() : undefined);

    return { description: text(input.description), agent: text(input.subagent_type) };
}

/** Three dots that orbit while the agent works and rest, coloured by how it ended, once it stops. */
export function DelegateOrbit(props: { status: DelegatedTaskStatus }) {
    return (
        <span class="delegate-orbit" data-status={props.status} aria-hidden="true">
            <i style={{ "--i": "0" }} />
            <i style={{ "--i": "1" }} />
            <i style={{ "--i": "2" }} />
        </span>
    );
}

/** What follows the title on the same line: a background mark, then agent, progress or queue, and time. */
export function DelegateDetails(props: {
    agent?: string;
    background: boolean;
    queued: boolean;
    progress: string | null;
    timing: ToolTimingState;
}) {
    const text = () =>
        [props.agent && agentLabel(props.agent), props.queued ? t("drift.task.queued") : props.progress]
            .filter(Boolean)
            .join(" · ");

    return (
        <span class="flex min-w-0 shrink items-center gap-1.5 text-xs text-ink-faint">
            <Show when={props.background}>
                <span class="flex shrink-0 text-accent/80" title={t("drift.task.background")}>
                    <IconLayers class="size-3.5" />
                </span>
            </Show>
            <Show when={text()}>
                <span class="min-w-0 truncate">{text()}</span>
            </Show>
            <ToolDuration state={props.timing} />
        </span>
    );
}
