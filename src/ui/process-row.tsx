import { openExternal } from "../shell";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Show } from "solid-js";

import type { ProcessRecord } from "../engine/processes";
import type { ProcessExitPart } from "../engine/parts";

/** A conversation's processes worth showing: all of them while any still runs, none once all have ended. */
export function dockProcesses(processes: readonly ProcessRecord[] | undefined) {
    const all = processes ?? [];
    if (!all.some((process) => process.state === "running")) return [];

    return [...all].sort((a, b) => b.startedAt - a.startedAt);
}

function tone(process: ProcessRecord) {
    if (process.state === "running") return "bg-accent animate-pulse";
    if (process.state === "stopped") return "bg-ink-faint";

    return process.exitCode === 0 ? "bg-ok" : "bg-danger";
}

function stateLabel(process: ProcessRecord) {
    if (process.state === "running") return t("drift.process.running");
    if (process.state === "stopped") return t("drift.process.stopped");

    return t("drift.process.exited", { code: process.exitCode ?? -1 });
}

/** One background process: what it is, how it is, its newest line, and Stop while it runs. */
export function ProcessRow(props: { process: ProcessRecord }) {
    const engine = useEngine();

    return (
        <li class="flex items-center gap-2 rounded-md px-2 py-1 text-xs text-ink-muted">
            <span class={`size-2 shrink-0 rounded-full ${tone(props.process)}`} />

            <span class="min-w-0 flex-1 truncate" title={props.process.command}>
                <span class="text-ink">{props.process.description}</span>
                <span class="text-ink-faint">
                    {" "}
                    · {t("drift.process.label")} · {stateLabel(props.process)}
                </span>
                <Show when={props.process.lastLine}>
                    {(line) => <span class="font-mono text-accent/80"> · {line()}</span>}
                </Show>
            </span>

            <Show when={props.process.url}>
                {(url) => (
                    <button
                        class="shrink-0 truncate text-accent/80 underline-offset-2 hover:underline"
                        title={url()}
                        onClick={() => openExternal(url())}
                    >
                        {t("drift.process.open")}
                    </button>
                )}
            </Show>

            <Show when={props.process.state === "running"}>
                <button
                    class="shrink-0 rounded-md border border-edge px-2 py-0.5 transition-colors hover:border-danger/50 hover:text-danger"
                    title={t("drift.process.stopHint")}
                    onClick={() => void engine.actions.stopProcess(props.process.id)}
                >
                    {t("drift.task.stop")}
                </button>
            </Show>
        </li>
    );
}

/** Why a turn started by itself: a background process ended, said as a row like a tool's. */
export function ProcessExitRow(props: { part: ProcessExitPart }) {
    const ended = () =>
        props.part.exitCode === undefined || props.part.exitCode === null
            ? t("drift.process.stopped")
            : t("drift.process.exited", { code: props.part.exitCode });

    return (
        <div
            class="flex min-h-8 w-full min-w-0 items-center justify-end gap-2 px-1.5 text-sm"
            title={props.part.output}
        >
            <span class={`size-2 shrink-0 rounded-full ${props.part.exitCode === 0 ? "bg-ok" : "bg-danger"}`} />
            <span class="shrink-0 font-medium text-ink-muted">{props.part.description}</span>
            <span class="min-w-0 truncate text-[0.85rem] text-ink-faint">{ended()}</span>
        </div>
    );
}
