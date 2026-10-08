import { backgroundRun, delegatedChildId, delegatedTaskClickPolicy, delegatedTaskStatus } from "./tool-delegation";
import { createEffect, createMemo, createSignal, For, on, Show, untrack } from "solid-js";
import { awaitingPermission, shellTimeoutStatus, toolInfo } from "./tool-labels";
import { toolMetadata as toolMeta } from "../engine/parts";
import { IconArrowUpRight, IconBranch } from "./icons";
import { toolErrorsExpanded } from "../state/prefs";
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

export function initialToolOpen(
    tool: string,
    status: ReturnType<typeof toolDisplay>["status"],
    errorsExpanded: boolean,
) {
    if (status === "error") return errorsExpanded;
    return tool === "bash";
}

const explicitToolOpen = new Map<string, boolean>();
const maxExplicitToolOpen = 1000;

export function initialToolOpenForPart(
    partId: string,
    tool: string,
    status: ReturnType<typeof toolDisplay>["status"],
    errorsExpanded: boolean,
) {
    return explicitToolOpen.get(partId) ?? initialToolOpen(tool, status, errorsExpanded);
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
        return `${count}${activity.current ? " · " + activity.current : ""}`;
    };
    const diff = () => {
        const value = toolMeta(props.part)?.diff;
        return typeof value === "string" && value.trim() ? value : null;
    };
    const error = () => (state().status === "error" ? (state() as { error: string }).error : null);
    const [open, setOpen] = createSignal(
        untrack(() => initialToolOpenForPart(props.part.id, props.part.name, state().status, toolErrorsExpanded())),
    );
    createEffect(
        on(error, (value, previous) => setOpen(nextToolOpen(open(), !!previous, !!value, toolErrorsExpanded()))),
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
        <div class="flex min-w-0 max-w-full flex-col gap-1 text-sm">
            <button
                class="flex min-h-8 w-full min-w-0 max-w-full items-center gap-2 overflow-hidden rounded-md px-1.5 text-left transition-colors hover:bg-raised/40"
                classList={{
                    "delegate-tool border-accent/35": delegated(),
                    "delegate-tool-background": !!background(),
                }}
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
                            fallback={<span class="shrink-0 font-semibold text-ink">{info().title}</span>}
                        >
                            <span class="shrink-0 font-semibold text-ink">
                                {t("drift.tool.called")}{" "}
                                <code class="rounded bg-raised px-1 font-mono text-xs font-normal">
                                    {info().called}
                                </code>
                            </span>
                        </Show>
                    }
                >
                    <TextShimmer text={title()} class="shrink-0 font-semibold" />
                </Show>
                <Show when={background()}>
                    <BackgroundTag />
                </Show>
                <Show when={info().subtitle && !(props.part.name === "bash" && expanded())}>
                    <span
                        class="min-w-0 truncate text-ink-faint"
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
                    <Chevron open={inlineExpanded()} />
                </Show>
            </button>
            <Show when={inlineExpanded()}>
                <div class="min-w-0 max-w-full">
                    <ToolBody part={props.part} diff={diff()} error={error()} />
                </div>
            </Show>
        </div>
    );
}

export function ExploredGroup(props: { parts: ToolPart[] }) {
    const engine = useEngine();
    const [open, setOpen] = createSignal(false);
    const label = () => `${t("settings.permissions.tool.read.title")} · ${props.parts.length}`;
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
        <div class="flex flex-col gap-1.5 text-sm">
            <button
                class="flex min-h-8 items-center gap-2 rounded-md px-1.5 text-left transition-colors hover:bg-raised/40"
                onClick={() => setOpen(!open())}
            >
                <Show when={waiting()}>
                    <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("notification.permission.title")} />
                </Show>
                <Show
                    when={!waiting() && running()}
                    fallback={<span class="font-semibold text-ink">{t("settings.permissions.tool.read.title")}</span>}
                >
                    <TextShimmer text={t("settings.permissions.tool.read.title")} class="font-semibold" />
                </Show>
                <span class="text-ink-faint">{label()}</span>
                <Show when={waiting()}>
                    <span class="text-xs text-warn/90">{t("notification.permission.title")}</span>
                </Show>
                <Show when={activePart()}>{(part) => <ToolDuration state={toolDisplay(part())} />}</Show>
                <Chevron open={expanded()} />
            </button>
            <Show when={expanded()}>
                <div class="ml-2 flex flex-col gap-0.5 border-l-2 border-edge pl-3">
                    <For each={props.parts}>{(part) => <ToolView part={part} />}</For>
                </div>
            </Show>
        </div>
    );
}
