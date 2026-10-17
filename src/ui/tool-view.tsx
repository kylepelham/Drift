import { backgroundRun, delegatedChildId, delegatedTaskClickPolicy, delegatedTaskStatus } from "./tool-delegation";
import { createEffect, createMemo, createSignal, For, on, onCleanup, Show, untrack } from "solid-js";
import { awaitingPermission, shellTimeoutStatus, toolInfo } from "./tool-labels";
import { collapseFinishedCommands, toolErrorsExpanded } from "../state/prefs";
import { exploredDetail, exploredSummary } from "./tool-run";
import { toolMetadata as toolMeta } from "../engine/parts";
import { IconArrowUpRight, IconBranch } from "./icons";
import { selectSession } from "../state/selection";
import { toolDisplay } from "./tool-presentation";
import { ToolDuration } from "./tool-duration";
import { taskTiming } from "../engine/store";
import { TextShimmer } from "./text-shimmer";
import { BackgroundTag } from "./task-dock";
import { parseDiff } from "./diff-panel";
import { ToolBody } from "./tool-body";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

import type { ToolPart } from "../engine/parts";

export function nextToolOpen(current: boolean, hadError: boolean, hasError: boolean, errorsExpanded: boolean) {
    return hasError && !hadError ? errorsExpanded : current;
}

/** How long a command runs before its live output opens, so quick ones never flash open. */
export const shellOpenDelayMs = 5000;

export function initialToolOpen(
    tool: string,
    status: ReturnType<typeof toolDisplay>["status"],
    errorsExpanded: boolean,
    collapseCommands = true,
    runningMs = 0,
) {
    if (status === "error") return errorsExpanded;
    if (tool !== "bash") return false;

    return !collapseCommands || (status === "running" && runningMs >= shellOpenDelayMs);
}

/** Milliseconds until a running command should open, or nothing when it never will. */
export function shellOpenWait(status: string, startedAt: number | undefined, now: number) {
    if (status !== "running" || startedAt === undefined) return;

    return Math.max(0, startedAt + shellOpenDelayMs - now);
}

const explicitToolOpen = new Map<string, boolean>();
const maxExplicitToolOpen = 1000;

export function initialToolOpenForPart(
    partId: string,
    tool: string,
    status: ReturnType<typeof toolDisplay>["status"],
    errorsExpanded: boolean,
    collapseCommands = true,
    runningMs = 0,
) {
    return explicitToolOpen.get(partId) ?? initialToolOpen(tool, status, errorsExpanded, collapseCommands, runningMs);
}

export function nextShellOpen(current: boolean, previous: string | undefined, status: string, explicit: boolean) {
    if (explicit || (previous !== "running" && previous !== "pending")) return current;
    return status === "running" || status === "pending" ? current : false;
}

export function rememberToolOpen(partId: string, open: boolean) {
    if (explicitToolOpen.get(partId) === open) return;

    explicitToolOpen.delete(partId);
    explicitToolOpen.set(partId, open);
    if (explicitToolOpen.size > maxExplicitToolOpen) explicitToolOpen.delete(explicitToolOpen.keys().next().value!);
}

export function activateToolHeader(toggle: () => void) {
    toggle();
}

export function openSpawnedThread(
    event: Pick<MouseEvent, "stopPropagation">,
    childId: string,
    select: (id: string) => void,
) {
    event.stopPropagation();
    select(childId);
}

export function toolChevronVisible(active: boolean, delegated: boolean) {
    return !active || delegated;
}

function diffStats(diff: string) {
    let additions = 0;
    let deletions = 0;
    for (const row of parseDiff(diff)) {
        if (row.kind === "add") additions++;
        if (row.kind === "del") deletions++;
    }

    return { additions, deletions };
}

export function ToolView(props: { part: ToolPart }) {
    const engine = useEngine();
    const state = () => toolDisplay(props.part);
    const info = () => toolInfo(props.part);
    const delegated = () => props.part.name === "task" || props.part.name === "spawn_thread";

    // Hoisted: the eager memo below runs before a const accessor would leave its dead zone.
    function spawnedId() {
        return delegatedChildId(engine.state, props.part);
    }

    // Memoized: it scans the parent transcript and is read from several places per delta.
    const delegatedStatus = createMemo(() => {
        if (!delegated()) return null;
        const childId = spawnedId();
        return childId ? delegatedTaskStatus(engine.state, props.part, childId) : null;
    });
    const background = createMemo(() => backgroundRun(engine.state, props.part));

    // A background launch returns at once; its time is the worker's own.
    const timing = () => {
        const task = background()?.task;
        return task ? taskTiming(task) : state();
    };

    const active = () => {
        if (awaitingPermission(engine.state, props.part)) return false;
        if (delegated()) return delegatedStatus() === "running";
        return state().status === "running" || state().status === "pending";
    };

    const title = () =>
        info().called ? `${t("drift.tool.called")} ${info().called}` : (info().title ?? props.part.name);

    const progress = () => {
        if (props.part.name !== "task") return null;

        const childId = spawnedId();
        if (!childId || delegatedStatus() !== "running") return null;

        const activity = engine.state.activity[childId];
        if (!activity) return null;

        const count = t(activity.tools === 1 ? "drift.count.tool.one" : "drift.count.tool.other", {
            count: activity.tools,
        });

        const current = activity.current ? " · " + activity.current : "";

        return `${count}${current}`;
    };

    const diff = () => {
        const value = toolMeta(props.part)?.diff;
        return typeof value === "string" && value.trim() ? value : null;
    };

    const error = () => (state().status === "error" ? (state() as { error: string }).error : null);
    const shellWait = () => {
        if (props.part.name !== "bash" || !collapseFinishedCommands()) return;
        return shellOpenWait(state().status, state().time.start, Date.now());
    };
    const [open, setOpen] = createSignal(
        untrack(() =>
            initialToolOpenForPart(
                props.part.id,
                props.part.name,
                state().status,
                toolErrorsExpanded(),
                collapseFinishedCommands(),
                Date.now() - (state().time.start ?? Date.now()),
            ),
        ),
    );
    createEffect(
        on(error, (value, previous) => setOpen(nextToolOpen(open(), !!previous, !!value, toolErrorsExpanded()))),
    );
    createEffect(() => {
        const wait = shellWait();
        if (!wait || explicitToolOpen.has(props.part.id)) return;

        const timer = setTimeout(() => {
            if (!explicitToolOpen.has(props.part.id)) setOpen(true);
        }, wait);
        onCleanup(() => clearTimeout(timer));
    });
    createEffect(
        on(
            () => state().status,
            (status, previous) => {
                if (props.part.name === "bash" && status !== "error" && untrack(collapseFinishedCommands))
                    setOpen(nextShellOpen(open(), previous, status, explicitToolOpen.has(props.part.id)));
            },
        ),
    );

    const expanded = () => open();

    const toggleOpen = () => {
        const next = !open();
        rememberToolOpen(props.part.id, next);
        setOpen(next);
    };

    const stats = () => {
        const patch = diff();
        return patch ? diffStats(patch) : null;
    };

    const timeout = () => shellTimeoutStatus(props.part);

    const activate = () => {
        if (delegatedTaskClickPolicy(delegatedStatus(), spawnedId()) === "navigate") {
            selectSession(spawnedId()!);
            return;
        }
        activateToolHeader(toggleOpen);
    };

    const inlineExpanded = () => expanded() && delegatedTaskClickPolicy(delegatedStatus(), spawnedId()) === "expand";

    return (
        <div class="tool-row flex min-w-0 max-w-full flex-col gap-1 text-sm">
            <button
                class="tool-row-header flex min-h-7 w-full min-w-0 max-w-full items-center gap-2 overflow-hidden rounded-md px-1 text-left transition-colors hover:bg-raised/35"
                classList={{
                    "delegate-tool border-accent/35": delegated(),
                    "delegate-tool-background": !!background(),
                }}
                aria-expanded={inlineExpanded()}
                onClick={activate}
            >
                <Show when={error()}>
                    <span class="size-3.5 shrink-0 text-danger">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="size-3.5">
                            <circle cx="12" cy="12" r="9" />
                            <path d="M5.5 5.5l13 13" />
                        </svg>
                    </span>
                </Show>
                <Show when={awaitingPermission(engine.state, props.part)}>
                    <span
                        class="size-1.5 shrink-0 rounded-full bg-warn"
                        title={t("drift.status.waitingForPermission")}
                    />
                </Show>
                <Show when={delegated()}>
                    <IconBranch class="delegate-tool-icon size-3.5 shrink-0 text-accent/70" />
                </Show>
                <Show
                    when={active()}
                    fallback={
                        <Show
                            when={info().called}
                            fallback={
                                <span
                                    class="tool-row-title"
                                    classList={{
                                        "shrink-0": props.part.name !== "bash",
                                        "min-w-0 truncate": props.part.name === "bash",
                                        "font-mono text-xs": !!info().titleMono,
                                    }}
                                >
                                    {info().title}
                                </span>
                            }
                        >
                            <span class="tool-row-title shrink-0">
                                {t("drift.tool.called")}{" "}
                                <code class="rounded bg-raised/70 px-1 font-mono text-xs font-normal">
                                    {info().called}
                                </code>
                            </span>
                        </Show>
                    }
                >
                    <TextShimmer
                        text={title()}
                        class={`tool-row-title ${props.part.name === "bash" ? "min-w-0 truncate" : "shrink-0"}${info().titleMono ? " font-mono text-xs" : ""}`}
                    />
                </Show>
                <Show when={background()}>
                    <BackgroundTag />
                </Show>
                <Show when={info().subtitle && !(props.part.name === "bash" && expanded())}>
                    <span
                        class="tool-row-subtitle min-w-0 truncate"
                        classList={{ "font-mono text-xs": info().mono, "text-[0.85rem]": !info().mono }}
                    >
                        {info().subtitle}
                    </span>
                </Show>
                <Show when={stats()}>
                    {(counts) => (
                        <span class="shrink-0 font-mono text-xs">
                            <span class="text-ok">+{counts().additions}</span>{" "}
                            <span class="text-danger">-{counts().deletions}</span>
                        </span>
                    )}
                </Show>
                <Show when={progress()}>
                    {(text) => <span class="shrink-0 font-mono text-xs text-accent/80">{text()}</span>}
                </Show>
                <Show when={timeout()}>
                    {(status) => (
                        <span
                            class="shrink-0 font-mono text-xs"
                            classList={{ "text-danger": status().timedOut, "text-ink-faint": !status().timedOut }}
                        >
                            {status().text}
                        </span>
                    )}
                </Show>
                <Show when={awaitingPermission(engine.state, props.part)}>
                    <span class="shrink-0 text-xs text-warn/90">{t("drift.status.waitingForPermission")}</span>
                </Show>
                <Show when={delegatedStatus() === "queued"}>
                    <span class="shrink-0 text-xs text-ink-faint" title={t("drift.task.queued.description")}>
                        {t("drift.task.queued")}
                    </span>
                </Show>
                <ToolDuration state={timing()} maxMs={timeout()?.timedOut ? timeout()?.timeoutMs : undefined} />
                <Show when={spawnedId()}>
                    {(childId) => (
                        <span
                            role="button"
                            title={t(
                                props.part.name === "task" ? "drift.thread.openSubagent" : "drift.thread.openSpawned",
                            )}
                            class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
                            onClick={(event) => openSpawnedThread(event, childId(), selectSession)}
                        >
                            <IconArrowUpRight class="size-3.5" />
                        </span>
                    )}
                </Show>
                <Show
                    when={
                        toolChevronVisible(active(), delegated()) &&
                        delegatedTaskClickPolicy(delegatedStatus(), spawnedId()) === "expand"
                    }
                >
                    <span class="tool-row-chevron flex shrink-0">
                        <Chevron open={inlineExpanded()} />
                    </span>
                </Show>
            </button>
            <Show when={inlineExpanded()}>
                <div class="tool-row-body min-w-0 max-w-full">
                    <ToolBody part={props.part} diff={diff()} error={error()} />
                </div>
            </Show>
        </div>
    );
}

export function ExploredGroup(props: { parts: ToolPart[] }) {
    const engine = useEngine();
    const [open, setOpen] = createSignal(false);
    const summary = createMemo(() => exploredSummary(props.parts, running()));
    const detail = createMemo(() => exploredDetail(props.parts));
    const waiting = () => props.parts.some((part) => awaitingPermission(engine.state, part));
    const running = () => props.parts.some((part) => part.status === "running" || part.status === "pending");

    const activePart = () => {
        for (let index = props.parts.length - 1; index >= 0; index--) {
            const part = props.parts[index];
            if (part.status === "running" || part.status === "pending") return part;
        }
    };

    const expanded = () => open() || waiting();

    return (
        <div class="tool-row flex flex-col gap-1.5 text-sm">
            <button
                class="tool-row-header flex min-h-7 min-w-0 items-center gap-2 overflow-hidden rounded-md px-1 text-left transition-colors hover:bg-raised/35"
                aria-expanded={expanded()}
                onClick={() => setOpen(!open())}
            >
                <Show when={waiting()}>
                    <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("notification.permission.title")} />
                </Show>
                <Show
                    when={!waiting() && running()}
                    fallback={<span class="tool-row-title shrink-0">{summary()}</span>}
                >
                    <TextShimmer text={summary()} class="tool-row-title shrink-0" />
                </Show>
                <Show when={detail()}>
                    <span class="tool-row-subtitle min-w-0 truncate">· {detail()}</span>
                </Show>
                <Show when={waiting()}>
                    <span class="text-xs text-warn/90">{t("notification.permission.title")}</span>
                </Show>
                <Show when={activePart()}>{(part) => <ToolDuration state={toolDisplay(part())} />}</Show>
                <span class="tool-row-chevron flex shrink-0">
                    <Chevron open={expanded()} />
                </span>
            </button>
            <Show when={expanded()}>
                <div class="tool-children">
                    <For each={props.parts}>{(part) => <ToolView part={part} />}</For>
                </div>
            </Show>
        </div>
    );
}
