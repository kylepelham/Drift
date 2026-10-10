import { createEffect, createMemo, createSignal, createUniqueId, For, on, Show } from "solid-js";
import { selectedSession, selectSession } from "../state/selection";
import { taskActive, type TaskRecord } from "../engine/store";
import { IconArrowUpRight } from "./icons";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

/** Marks a subagent that runs in the background; the dashed edge matches its row in the chat. */
export function BackgroundTag() {
    return (
        <span class="shrink-0 rounded border border-dashed border-accent/45 px-1 text-[0.65rem] leading-4 text-accent/80">
            {t("drift.task.background")}
        </span>
    );
}

/** Background workers worth showing, in dock order: all of them while any is still going or owed. */
export function dockTasks(tasks: readonly TaskRecord[] | undefined) {
    const background = (tasks ?? []).filter((task) => task.mode === "background");
    const outstanding = background.some((task) => taskActive(task) || !task.delivered);
    if (!outstanding) return [];

    return background.sort(byDockOrder);
}

/** Delivered results, the rank the dock folds away as history. */
const HISTORY = 3;

/** Running work first, then work waiting for a slot, then results still owed, then history. */
function taskRank(task: TaskRecord) {
    if (task.state === "running") return 0;
    if (task.state === "queued") return 1;

    return task.delivered ? HISTORY : 2;
}

/** Within a rank, live work in launch order and history newest first. */
function byDockOrder(a: TaskRecord, b: TaskRecord) {
    const rank = taskRank(a) - taskRank(b);
    if (rank !== 0) return rank;

    const age = taskRank(a) === HISTORY ? b.createdAt - a.createdAt : a.createdAt - b.createdAt;
    return age || a.id.localeCompare(b.id);
}

const stateTone: Record<TaskRecord["state"], string> = {
    queued: "border border-edge-strong",
    running: "bg-accent animate-pulse",
    replied: "bg-ok",
    failed: "bg-danger",
    stopped: "bg-ink-faint",
    interrupted: "bg-warn",
};

export function TaskDock() {
    const engine = useEngine();
    const [open, setOpen] = createSignal(false);
    const bodyID = `task-dock-${createUniqueId()}`;

    const tasks = createMemo(() => dockTasks(engine.state.tasks[selectedSession() ?? ""]));
    const outstanding = createMemo(() => tasks().filter((task) => taskActive(task) || !task.delivered));
    const history = createMemo(() => tasks().filter((task) => !taskActive(task) && task.delivered));
    const finished = () => tasks().filter((task) => !taskActive(task)).length;
    const current = () => tasks().find((task) => task.state === "running") ?? tasks().find(taskActive);

    // Another conversation's dock starts closed.
    createEffect(on(selectedSession, () => setOpen(false)));

    return (
        <Show when={tasks().length > 0}>
            <div class="composer-layer-card dock-card min-w-0 rounded-lg border border-edge bg-surface text-sm">
                <button
                    class="flex w-full min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap px-3 py-1.5 text-ink-muted"
                    aria-expanded={open()}
                    aria-controls={bodyID}
                    onClick={() => setOpen(!open())}
                >
                    <Chevron open={open()} />
                    <span class="min-w-0 truncate text-left">
                        {t("drift.task.title")} ·{" "}
                        {t("drift.task.progress", { done: finished(), total: tasks().length })}
                    </span>
                    <Show when={!open()}>
                        <span class="min-w-0 flex-1 truncate text-left text-ink-faint">{current()?.description}</span>
                    </Show>
                </button>

                <Show when={open()}>
                    <div id={bodyID} class="border-t border-edge">
                        <ul
                            class="max-h-[min(12rem,24dvh)] space-y-0.5 overflow-y-auto overscroll-contain px-2 py-2"
                            tabIndex={0}
                            aria-label={t("drift.task.title")}
                        >
                            <For each={outstanding()}>{(task) => <TaskRow task={task} />}</For>
                        </ul>

                        <Show when={history().length > 0}>
                            <FinishedTasks tasks={history()} id={`${bodyID}-history`} />
                        </Show>
                    </div>
                </Show>
            </div>
        </Show>
    );
}

/** Delivered results, folded away by default and scrolled separately so they never push live work out of view. */
function FinishedTasks(props: { tasks: TaskRecord[]; id: string }) {
    const [open, setOpen] = createSignal(false);

    // Another conversation's history starts folded.
    createEffect(on(selectedSession, () => setOpen(false)));

    return (
        <div class="border-t border-edge">
            <button
                class="flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left text-xs text-ink-faint hover:text-ink-muted"
                aria-expanded={open()}
                aria-controls={props.id}
                onClick={() => setOpen(!open())}
            >
                <Chevron open={open()} />
                <span class="min-w-0 truncate">
                    {t("drift.task.finished")} · {props.tasks.length}
                </span>
            </button>

            <Show when={open()}>
                <ul
                    id={props.id}
                    class="max-h-[min(8rem,16dvh)] space-y-0.5 overflow-y-auto overscroll-contain px-2 pb-2"
                    tabIndex={0}
                    aria-label={t("drift.task.finished")}
                >
                    <For each={props.tasks}>{(task) => <TaskRow task={task} />}</For>
                </ul>
            </Show>
        </div>
    );
}

function TaskRow(props: { task: TaskRecord }) {
    const engine = useEngine();
    const activity = () =>
        props.task.state === "running" ? engine.state.activity[props.task.sessionId]?.current : undefined;

    return (
        <li
            class="flex items-center gap-2 rounded-md px-2 py-1 text-xs text-ink-muted"
            title={props.task.delivered ? undefined : (props.task.deliveryError ?? undefined)}
        >
            <span class={`size-2 shrink-0 rounded-full ${stateTone[props.task.state]}`} />

            <span class="min-w-0 flex-1 truncate" title={props.task.description}>
                <span class="text-ink">{props.task.description}</span>
                <span class="text-ink-faint">
                    {" "}
                    · @{props.task.agent} · {t(`drift.task.state.${props.task.state}`)}
                </span>
                <Show when={props.task.held && !props.task.delivered}>
                    <span class="text-ink-faint"> · {t("drift.task.held")}</span>
                </Show>
                <Show when={!props.task.delivered && props.task.deliveryError}>
                    {(reason) => <span class="text-warn"> · {t("drift.task.owed", { reason: reason() })}</span>}
                </Show>
                <Show when={activity()}>{(text) => <span class="font-mono text-accent/80"> · {text()}</span>}</Show>
            </span>

            <Show when={taskActive(props.task)}>
                <button
                    class="shrink-0 rounded-md border border-edge px-2 py-0.5 transition-colors hover:border-danger/50 hover:text-danger"
                    title={t("drift.task.stopHint")}
                    onClick={() => void engine.actions.stopTask(props.task.id)}
                >
                    {t("drift.task.stop")}
                </button>
            </Show>

            <button
                class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
                title={t("drift.thread.openSubagent")}
                aria-label={t("drift.thread.openSubagent")}
                onClick={() => selectSession(props.task.sessionId)}
            >
                <IconArrowUpRight class="size-3.5" />
            </button>
        </li>
    );
}
