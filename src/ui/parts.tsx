import { codeTokens, Markdown, openWorkspaceFile, ProgressiveCodeView, type SyntaxToken } from "./markdown";
import { childrenOf, taskActive, taskForCall, taskTiming, type EngineState } from "../engine/store";
import { IconArrowUpRight, IconBranch, IconCheck, IconCopy, IconInfo, IconPlug } from "./icons";
import { hasPartRenderer, hasToolRenderer, PluginPartView, PluginToolView } from "../plugins";
import { diffIndicator, diffLineNumbers, diffWordWrap, syntaxTheme } from "../state/code";
import { splitOrchestratorStatus, type OrchestratorStatus } from "../state/orchestrator";
import { promptPartText, toolInput, toolMetadata } from "../engine/parts";
import { showReasoning, toolErrorsExpanded } from "../state/prefs";
import { openToolContextMenu } from "./tool-context-menu";
import { resolveFileLanguage } from "../syntax-language";
import { classifyMarkdownLink } from "./markdown-links";
import { resolveAttachmentKind } from "../attachments";
import { citationFileGroups } from "./citation-files";
import { selectSession } from "../state/selection";
import { toolDisplay } from "./tool-presentation";
import { ToolDuration } from "./tool-duration";
import { formatBytes } from "../state/storage";
import { TextShimmer } from "./text-shimmer";
import { BackgroundTag } from "./task-dock";
import { openFile } from "../tool-actions";
import { openLightbox } from "./lightbox";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";
import {
    createEffect,
    createMemo,
    createSignal,
    For,
    Match,
    on,
    onCleanup,
    onMount,
    Show,
    Switch,
    untrack,
    type JSX,
} from "solid-js";
import {
    awaitingPermission,
    patchFiles,
    shellTimeoutStatus,
    toolFilename,
    toolInfo,
    type PatchFile,
} from "./tool-labels";

import type { FilePart, Part, ContextPart, ReasoningPart, ToolPart } from "../engine/parts";
import type { TaskRecord } from "../engine/store";

const hiddenTools = new Set(["todowrite", "todoread"]);

// How long the shell copy button shows its "copied" state.
// NOTE: markdown.tsx uses 1600ms for its visually identical code-block copy button.
const copiedFeedbackMs = 2000;
// Tool output beyond this is clipped before rendering; long outputs otherwise stall the view.
const maxInlineOutputChars = 4000;
// When re-syncing streamed shell output, compare this many trailing characters of the previous
// chunk against the new one to confirm the stream is an append rather than a fresh transcript.
const overlapProbeChars = 64;
// Scroll positions within this many pixels of the bottom count as "at the bottom".
const bottomSlopPx = 2;
// Shiki packs font styling into a bitmask on each token; these are its FontStyle enum values.
const fontStyleItalic = 1;
const fontStyleBold = 2;
const fontStyleUnderline = 4;

/** `orchestrated`: the orchestrator wrote the reply, so its status block shows as a row; from any other agent it is only hidden. */
export function PartView(props: {
    part: Part;
    responseID?: string;
    live?: boolean;
    revision?: number;
    thinking?: boolean;
    orchestrated?: boolean;
}) {
    const engine = useEngine();
    return (
        <Switch>
            <Match when={props.part.type !== "tool_call" && hasPartRenderer(props.part.type) && props.part}>
                {(part) => <PluginPartView part={part()} />}
            </Match>
            <Match when={visibleText(props.part)}>
                {(part) => {
                    const split = createMemo(() => splitOrchestratorStatus(part().text));
                    return (
                        <>
                            <Show when={split().prose}>
                                <Markdown
                                    text={split().prose}
                                    directory={engine.state.sessions[part().part.sessionId]?.directory}
                                    fileGroups={() =>
                                        citationFileGroups(
                                            engine.state,
                                            part().part.sessionId,
                                            part().part.messageId,
                                            part().part.id,
                                        )
                                    }
                                    done={false}
                                    responseID={props.responseID}
                                    live={props.live}
                                    revision={props.revision}
                                />
                            </Show>
                            <Show when={props.orchestrated && split().status}>
                                {(status) => <OrchestratorStatusRow status={status()} />}
                            </Show>
                        </>
                    );
                }}
            </Match>
            <Match when={props.part.type === "context" && props.part}>{(part) => <PluginRow part={part()} />}</Match>
            <Match when={showReasoning() && props.part.type === "reasoning" && (props.part as ReasoningPart)}>
                {(part) => <ReasoningView part={part()} revision={props.revision} />}
            </Match>
            <Match
                when={
                    props.part.type === "tool_call" &&
                    hasToolRenderer((props.part as ToolPart).name) &&
                    (props.part as ToolPart)
                }
            >
                {(part) => (
                    <ToolContextTarget part={part()}>
                        <PluginToolView part={part()} />
                    </ToolContextTarget>
                )}
            </Match>
            <Match
                when={
                    props.part.type === "tool_call" &&
                    !hiddenTools.has((props.part as ToolPart).name) &&
                    (props.part as ToolPart)
                }
            >
                {(part) => (
                    <ToolContextTarget part={part()}>
                        <ToolView part={part()} />
                    </ToolContextTarget>
                )}
            </Match>
            <Match when={props.part.type === "compaction"}>
                <div class="my-2 flex items-center gap-3 text-xs text-ink-faint">
                    <div class="h-px flex-1 bg-edge" />
                    <TextShimmer
                        text={props.thinking ? t("drift.context.compacting") : t("drift.context.compacted")}
                        active={!!props.thinking}
                    />
                    <div class="h-px flex-1 bg-edge" />
                </div>
            </Match>
            <Match when={props.part.type === "file" && props.part}>
                {(part) => <FilePartView part={part() as FilePart} />}
            </Match>
        </Switch>
    );
}

function ToolContextTarget(props: { part: ToolPart; children: JSX.Element }) {
    return <div onContextMenu={(event) => openToolContextMenu(event, props.part)}>{props.children}</div>;
}

function visibleText(part: Part) {
    const text = promptPartText(part);
    if (text === undefined) return undefined;

    const split = splitOrchestratorStatus(text);
    return split.prose.trim() || split.status ? { part, text } : undefined;
}

const statusLabels = {
    working: "drift.orchestrator.state.working",
    done: "drift.orchestrator.state.done",
    blocked: "drift.orchestrator.state.blocked",
} as const;

/** The orchestrator's end-of-reply status, as a row like a tool's rather than the JSON it wrote; on the
 * user's side, since while it says Working the engine prompts again on the user's behalf. */
function OrchestratorStatusRow(props: { status: OrchestratorStatus }) {
    return (
        <div class="flex min-h-8 min-w-0 items-center justify-end gap-2 px-1.5 text-sm">
            <Switch>
                <Match when={props.status.state === "done"}>
                    <IconCheck class="size-3.5 shrink-0 text-ok" />
                </Match>
                <Match when={props.status.state === "blocked"}>
                    <span class="size-1.5 shrink-0 rounded-full bg-warn" />
                </Match>
                <Match when={props.status.state === "working"}>
                    <IconArrowUpRight class="size-3.5 shrink-0 text-accent/70" />
                </Match>
            </Switch>
            <span class="shrink-0 font-semibold text-ink">{t(statusLabels[props.status.state])}</span>
            <Show when={props.status.headline}>
                <span class="min-w-0 truncate text-[0.85rem] text-ink-faint" title={props.status.headline}>
                    {props.status.headline}
                </span>
            </Show>
        </div>
    );
}

/** What a plugin said, as a row like a tool's: its name, then its words on one line. */
export function PluginRow(props: { part: ContextPart; end?: boolean }) {
    return (
        <div
            class="flex min-h-8 min-w-0 items-center gap-2 px-1.5 text-sm"
            classList={{ "w-full justify-end": props.end }}
            title={props.part.text}
        >
            <IconPlug class="size-3.5 shrink-0 text-ink-faint" />
            <span class="shrink-0 font-medium text-ink-muted">{props.part.plugin}</span>
            <span class="min-w-0 truncate text-[0.85rem] text-ink-faint">{props.part.text}</span>
        </div>
    );
}

export function partVisible(part: Part) {
    if (part.type !== "tool_call" && hasPartRenderer(part.type)) return true;
    switch (part.type) {
        case "text":
        case "nudge":
        case "clarification":
            return !!visibleText(part);
        case "reasoning":
            return showReasoning();
        case "tool_call":
            return !hiddenTools.has(part.name);
        case "compaction":
        case "file":
        case "context":
            return true;
        default:
            return false;
    }
}

export function FilePartView(props: { part: Pick<FilePart, "mime" | "name" | "url" | "path">; directory?: string }) {
    const linkable = () => props.part.url.startsWith("data:") || props.part.url.startsWith("http");
    const resolved = () => resolveAttachmentKind({ filename: props.part.name, mime: props.part.mime });
    const kind = () => resolved().kind;
    const mention = () => (props.directory ? props.part.path : undefined);
    return (
        <Show when={mention()} fallback={<AttachmentView part={props.part} linkable={linkable()} kind={kind()} />}>
            {(path) => <MentionChip path={path()} directory={props.directory!} kind={kind()} />}
        </Show>
    );
}

/** An `@` mention: opens the file it names, as a file link in a reply would. */
function MentionChip(props: { path: string; directory: string; kind: string }) {
    const open = () => {
        const link = classifyMarkdownLink(props.path, props.directory);
        if (link.kind === "file") void openWorkspaceFile(link, props.directory);
    };
    return (
        <button
            type="button"
            title={props.path}
            class="inline-flex max-w-full items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
            onClick={open}
        >
            <AttachmentFileLabel filename={props.path} kind={props.kind === "unsupported" ? "text" : props.kind} bare />
        </button>
    );
}

function AttachmentView(props: { part: Pick<FilePart, "mime" | "name" | "url">; linkable: boolean; kind: string }) {
    const linkable = () => props.linkable;
    const kind = () => props.kind;
    return (
        <Switch
            fallback={
                <Show
                    when={linkable()}
                    fallback={
                        <span class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted">
                            <span class="truncate">{props.part.name ?? t("common.attachment")}</span>
                        </span>
                    }
                >
                    <a
                        href={props.part.url}
                        download={props.part.name ?? "attachment"}
                        title={t("drift.attachment.download")}
                        class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-edge bg-raised px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                    >
                        <span class="truncate">{props.part.name ?? t("common.attachment")}</span>
                    </a>
                </Show>
            }
        >
            <Match when={kind() === "image" && linkable()}>
                <ImageThumb url={props.part.url} filename={props.part.name} mime={props.part.mime} />
            </Match>
            <Match when={kind() === "audio" && linkable()}>
                <audio controls src={props.part.url} class="max-w-full" />
            </Match>
            <Match when={kind() === "video" && linkable()}>
                <video controls src={props.part.url} class="max-h-64 max-w-full rounded-lg border border-edge" />
            </Match>
            <Match when={(kind() === "pdf" || kind() === "text" || kind() === "csv") && kind()}>
                {(attachmentKind) => (
                    <Show
                        when={linkable()}
                        fallback={<AttachmentFileLabel filename={props.part.name} kind={attachmentKind()} />}
                    >
                        <a
                            href={props.part.url}
                            download={props.part.name ?? "attachment"}
                            title={t("drift.attachment.download")}
                            class="inline-flex max-w-full items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                        >
                            <AttachmentFileLabel filename={props.part.name} kind={attachmentKind()} bare />
                        </a>
                    </Show>
                )}
            </Match>
        </Switch>
    );
}

function AttachmentFileLabel(props: { filename?: string; kind: string; bare?: boolean }) {
    return (
        <span
            class="inline-flex min-w-0 max-w-full items-center gap-2"
            classList={{
                "rounded-md border border-edge bg-raised py-1 pr-2 pl-1.5 text-xs text-ink-muted": !props.bare,
            }}
        >
            <span class="rounded bg-overlay px-1 py-0.5 font-mono text-[0.6rem] font-semibold text-accent uppercase">
                {t(`drift.attachment.kind.${props.kind}`)}
            </span>
            <span class="truncate">{props.filename ?? t("common.attachment")}</span>
        </span>
    );
}

function ImageThumb(props: { url: string; filename?: string; mime?: string }) {
    return (
        <button
            title={props.filename ?? t("drift.attachment.viewImage")}
            class="block overflow-hidden rounded-md border border-edge transition-colors hover:border-edge-strong"
            onClick={() => openLightbox({ url: props.url, filename: props.filename, mime: props.mime })}
        >
            <img src={props.url} alt={props.filename ?? t("drift.attachment.image")} class="size-20 object-cover" />
        </button>
    );
}

function ReasoningView(props: { part: ReasoningPart; revision?: number }) {
    const engine = useEngine();
    const [open, setOpen] = createSignal(false);
    return (
        <div class="text-sm">
            <button
                class="flex items-center gap-1.5 text-ink-faint transition-colors hover:text-ink-muted"
                onClick={() => setOpen(!open())}
            >
                <Chevron open={open()} />
                <TextShimmer text={t("drift.reasoning.thinking")} active />
            </button>
            <Show when={open()}>
                <div class="mt-1.5 border-l-2 border-edge pl-3 text-ink-muted">
                    <Markdown
                        text={props.part.text}
                        directory={engine.state.sessions[props.part.sessionId]?.directory}
                        fileGroups={() =>
                            citationFileGroups(engine.state, props.part.sessionId, props.part.messageId, props.part.id)
                        }
                        done={false}
                        revision={props.revision}
                    />
                </div>
            </Show>
        </div>
    );
}

function toolMeta(part: ToolPart) {
    return toolMetadata(part);
}

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

export function delegatedTaskClickPolicy(status: DelegatedTaskStatus | null, childId: string | null) {
    return childId && status === "running" ? "navigate" : "expand";
}

export function delegatedChildId(state: EngineState, part: ToolPart) {
    if (part.name !== "task" && part.name !== "spawn_thread") return null;
    const sessionId = (toolMeta(part) as { sessionId?: unknown } | undefined)?.sessionId;
    if (typeof sessionId === "string" && sessionId) return sessionId;
    if (part.name !== "task") return null;

    const input = toolInput(part) as { description?: unknown; subagent_type?: unknown; task_id?: unknown };
    if (typeof input?.task_id === "string" && input.task_id) return input.task_id;
    if (typeof input?.description !== "string" || typeof input.subagent_type !== "string") return null;

    // Parallel tasks can create their child before the running tool part persists its session metadata.
    const title = `${input.description} (@${input.subagent_type} subagent)`;
    const matches = childrenOf(state, part.sessionId).filter((session) => session.title === title);
    return matches.length === 1 ? matches[0].id : null;
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
    // A hoisted declaration: delegatedStatus below is an eager memo, and a `const` accessor here
    // would still be in its temporal dead zone during the first evaluation, crashing every
    // transcript that contains a delegated task row.
    function spawnedId() {
        return delegatedChildId(engine.state, props.part);
    }
    // Memoized: this scans the parent transcript for terminal markers and is read from half a dozen
    // reactive positions per tool row; unmemoized it re-ran the scan for each of them per delta.
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

export type DelegatedTaskStatus = "running" | "completed" | "error";

/** A `task` call whose worker runs in the background: the engine's record, or before it arrives, what the call says. */
export function backgroundRun(state: EngineState, part: ToolPart) {
    if (part.name !== "task") return null;
    const metadata = part.metadata;
    const task = taskForCall(state, part.sessionId, part.callId, metadata?.taskId);
    if (task) return task.mode === "background" ? { task } : null;
    const asked = toolInput(part).run_in_background === true;
    return asked || metadata?.background === true || metadata?.mode === "background" ? { task: undefined } : null;
}

export function delegatedTaskStatus(state: EngineState, part: ToolPart, childId: string): DelegatedTaskStatus {
    // This invocation's result stays terminal even when another call resumes the same child.
    if (part.status === "error" || part.status === "denied") return "error";
    if (part.name === "spawn_thread") return part.status === "done" ? "completed" : "running";
    // The engine's record outranks the call: a background call finishes at launch, its worker later.
    const task = taskForCall(state, part.sessionId, part.callId, part.metadata?.taskId);
    if (task) return delegatedRecordStatus(task);
    const terminal = delegatedTerminalState(state, part, childId);
    if (terminal) return terminal;
    return state.errors[childId] ? "error" : "running";
}

function delegatedRecordStatus(task: Pick<TaskRecord, "state">): DelegatedTaskStatus {
    if (taskActive(task)) return "running";

    return task.state === "replied" ? "completed" : "error";
}

function delegatedTerminalState(
    state: EngineState,
    part: ToolPart,
    childId: string,
): "completed" | "error" | undefined {
    if (part.status !== "done") return;
    const pattern = new RegExp(
        `^\\s*<task\\s+id=["']${escapeRegExp(childId)}["']\\s+state=["'](running|completed|error)["']`,
    );
    const result = (part.output ?? "").match(pattern)?.[1];
    if (result === "completed" || result === "error") return result;
    const background = part.metadata?.background === true || part.metadata?.mode === "background";
    if (part.name === "task" && result !== "running" && !background) return "completed";

    return followingTaskResult(state.transcripts[part.sessionId] ?? [], part.id, pattern);
}

function followingTaskResult(entries: EngineState["transcripts"][string], partID: string, pattern: RegExp) {
    // Background calls finish their tool part before the work. Find their first later result,
    // never an earlier invocation's result or a later foreground call's output.
    let after = false;
    for (const entry of entries) {
        for (const item of entry.parts) {
            if (item.id === partID) after = true;
            if (!after || item.type !== "text") continue;
            const match = item.text.match(pattern)?.[1];
            if (match === "completed" || match === "error") return match;
        }
    }
}

function escapeRegExp(value: string) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function ToolBody(props: { part: ToolPart; diff: string | null; error: string | null }) {
    const engine = useEngine();
    const state = () => toolDisplay(props.part);
    const shellCommand = () => (state().input as { command?: string }).command ?? "";
    // While running, the output so far rides on the part's metadata; once ended, the saved result is the output.
    const shellOutput = () => {
        const current = state();
        if (current.status === "completed") return current.output;
        if (current.status === "error") return current.error;
        return (toolMeta(props.part)?.output as string | undefined) ?? "";
    };
    const shell = createMemo(() => splitNotes(shellOutput() ?? "", toolMeta(props.part)?.notes));
    const written = () => {
        if (props.part.name !== "write") return null;
        const input = state().input as { content?: string; filePath?: string };
        return typeof input.content === "string"
            ? { content: input.content, name: toolFilename(input.filePath) }
            : null;
    };
    const patched = () => (props.part.name === "apply_patch" ? patchFiles(props.part) : []);
    const diffFilename = () => {
        const input = state().input as { filePath?: string };
        const file = patched()[0];
        return file?.relativePath ?? file?.filePath ?? input.filePath ?? "";
    };
    const tasked = () => taskBody(props.part);
    const citationFiles = () => {
        const child = delegatedChildId(engine.state, props.part);
        return child
            ? citationFileGroups(
                  engine.state,
                  child,
                  undefined,
                  undefined,
                  props.part.status === "done"
                      ? (props.part.finishedAt ?? props.part.startedAt ?? undefined)
                      : undefined,
              )
            : citationFileGroups(engine.state, props.part.sessionId, props.part.messageId, props.part.id);
    };
    return (
        <>
            <Switch fallback={<GenericBody part={props.part} />}>
                <Match when={tasked()}>
                    {(task) => (
                        <div class="space-y-2 border-l-2 border-edge pl-3">
                            <Show when={task().prompt}>
                                <div class="transcript-tool-output max-h-40 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-faint">
                                    {clip(task().prompt)}
                                </div>
                            </Show>
                            <Show when={task().result}>
                                <div class="transcript-tool-output max-h-80 overflow-auto text-ink-muted">
                                    <Markdown
                                        text={task().result}
                                        directory={
                                            engine.state.sessions[
                                                delegatedChildId(engine.state, props.part) ?? props.part.sessionId
                                            ]?.directory
                                        }
                                        fileGroups={citationFiles}
                                        done
                                    />
                                </div>
                            </Show>
                        </div>
                    )}
                </Match>
                <Match when={props.part.name === "bash"}>
                    <ShellOutput
                        command={shellCommand()}
                        output={shell().output}
                        file={toolMeta(props.part)?.outputFile as string | undefined}
                        running={state().status === "pending" || state().status === "running"}
                    />
                    <For each={shell().notes}>
                        {(note) => (
                            <div class="mt-1 flex items-start gap-1.5 px-1 text-xs text-ink-faint">
                                <IconInfo class="mt-0.5 size-3 shrink-0" />
                                <span class="min-w-0 whitespace-pre-wrap">{note}</span>
                            </div>
                        )}
                    </For>
                </Match>
                <Match when={written()}>
                    {(file) => <ProgressiveCodeView code={file().content} filename={file().name ?? ""} />}
                </Match>
                <Match when={patched().length > 1 && patched()}>{(files) => <PatchPanel files={files()} />}</Match>
                <Match when={props.diff}>{(patch) => <DiffPanel diff={patch()} filename={diffFilename()} />}</Match>
            </Switch>
            <Show when={props.error}>
                {(message) => (
                    <div class="mt-1.5 border-l-2 border-danger py-0.5 pl-3 text-[0.85rem] break-words whitespace-pre-wrap text-ink-muted">
                        {clip(stripAnsi(message()))}
                    </div>
                )}
            </Show>
        </>
    );
}

/** A call's output without the notes Drift appended (`metadata.notes`), and those notes, shown under the call instead. */
export function splitNotes(output: string, notes: unknown): { output: string; notes: string[] } {
    const listed = Array.isArray(notes) ? notes.filter((note): note is string => typeof note === "string") : [];
    let rest = output;
    const shown: string[] = [];
    for (const note of [...listed].reverse()) {
        if (rest === note) rest = "";
        else if (rest.endsWith(`\n\n${note}`)) rest = rest.slice(0, rest.length - note.length - 2);
        else break;
        shown.unshift(note);
    }
    return { output: rest, notes: shown };
}

/** The line the engine puts where it cut a long output's middle out (`tool::spool`), and how much it cut. */
export function splitOmitted(text: string): { head: string; omitted: number; tail: string } | null {
    const found = /\n\n\.\.\. (\d+) bytes omitted; [^\n]* \.\.\.\n\n/.exec(text);
    if (!found) return null;
    return {
        head: text.slice(0, found.index),
        omitted: Number(found[1]),
        tail: text.slice(found.index + found[0].length),
    };
}

export function shellTranscript(command: string, output: string) {
    const normalized = stripAnsi(output).replace(/\r\n?/g, "\n");
    return `$ ${command}${normalized.trim() ? `\n\n${normalized}` : ""}`;
}

/**
 * Splits a replace-frame transcript into its command line and trailing output so the command can
 * be rendered with the accent `$ ` indicator. Replace frames always start with `$ ${command}`;
 * the fallback keeps unexpected frames rendering verbatim instead of dropping text.
 */
export function shellReplaceSegments(command: string, text: string) {
    const head = `$ ${command}`;
    if (!text.startsWith(head)) return { command: null, output: text };
    return { command, output: text.slice(head.length) };
}

/**
 * Renders streaming shell output incrementally.
 *
 * The engine re-sends the whole output on every update. Re-rendering all of it each time is too
 * slow for a long-running command, so this tracks how much has already been shown and emits only
 * the new tail when it can prove the new output is an append of the old one. When it cannot, it
 * emits a full replacement.
 *
 * Callers get `{ replace, text }`: `replace: true` means "this is the whole transcript",
 * `replace: false` means "append this".
 */
export function createShellTranscriptStream() {
    let command = "";
    /** How many characters of the raw output have already been turned into visible text. */
    let outputLength = 0;
    let previousOutput = "";
    let initialized = false;
    /** Set once non-whitespace output has been shown; until then output is buffered in `pending`. */
    let visible = false;
    let finished = false;
    /** ANSI escape parser state. CSI sequences run until a byte in the 0x40-0x7e final range. */
    let escape: "none" | "start" | "csi" = "none";
    /** A trailing CR is held over: it only becomes a newline once we know what follows it. */
    let carriageReturn = false;
    /** Whitespace-only chunks seen before the first visible output, kept so none are lost. */
    let pending: string[] = [];

    /** Strips ANSI escapes and folds CR / CRLF into newlines. `flush` resolves a trailing CR. */
    const consume = (value: string, flush: boolean) => {
        let normalized = "";
        for (const character of value) {
            if (escape === "start") {
                escape = character === "[" ? "csi" : "none";
                continue;
            }
            if (escape === "csi") {
                const code = character.charCodeAt(0);
                if (code >= 0x40 && code <= 0x7e) escape = "none";
                continue;
            }
            if (character === "\u001b") {
                escape = "start";
                continue;
            }
            if (carriageReturn) {
                normalized += "\n";
                carriageReturn = false;
                if (character === "\n") continue;
            }
            if (character === "\r") carriageReturn = true;
            else normalized += character;
        }
        if (flush && carriageReturn) {
            normalized += "\n";
            carriageReturn = false;
        }
        return normalized;
    };

    /** Re-renders the whole transcript from scratch and re-primes the incremental state. */
    const reset = (nextCommand: string, output: string, done: boolean) => {
        command = nextCommand;
        outputLength = output.length;
        previousOutput = output;
        initialized = true;
        finished = done;
        visible = false;
        escape = "none";
        carriageReturn = false;
        pending = [];
        const normalized = consume(output, done);
        visible = !!normalized.trim();
        if (!visible) pending = [normalized];
        return { replace: true, text: `$ ${command}${visible ? `\n\n${normalized}` : ""}` };
    };

    return {
        update(nextCommand: string, output: string, done: boolean) {
            // Nothing to append against: first update, a different command, output that shrank, or a
            // final frame whose trailing CR still has to be flushed.
            if (!initialized || command !== nextCommand || output.length < outputLength || finished || done) {
                return reset(nextCommand, output, done);
            }
            // Same length: either a genuine no-op, or the content changed underneath us.
            if (output.length === outputLength) {
                if (output === previousOutput) return { replace: false, text: "" };
                return reset(nextCommand, output, done);
            }
            // The engine truncated the head and prefixed an ellipsis, so earlier offsets no longer line up.
            if (output.startsWith("...\n\n") && !previousOutput.startsWith("...\n\n"))
                return reset(nextCommand, output, done);
            // Confirm this really is an append: the tail of what we last saw must still sit at the same
            // offset. If it does not, the output was rewritten rather than extended.
            const overlap = previousOutput.slice(-overlapProbeChars);
            if (output.slice(outputLength - overlap.length, outputLength) !== overlap)
                return reset(nextCommand, output, done);

            const normalized = consume(output.slice(outputLength), false);
            outputLength = output.length;
            previousOutput = output;
            if (visible) return { replace: false, text: normalized };
            // Still nothing but whitespace so far. Hold it back rather than opening the transcript with
            // blank lines, and emit the whole block at once as soon as real output arrives.
            pending.push(normalized);
            const combined = pending.join("");
            if (!combined.trim()) return { replace: false, text: "" };
            visible = true;
            pending = [];
            return { replace: true, text: `$ ${command}\n\n${combined}` };
        },
    };
}

export function createFrameCoalescer<T>(
    schedule: (callback: () => void) => number,
    cancel: (handle: number) => void,
    apply: (value: T) => void,
) {
    let frame: number | undefined;
    let latest: T;
    return {
        push(value: T, defer: boolean) {
            latest = value;
            if (!defer) {
                if (frame !== undefined) cancel(frame);
                frame = undefined;
                apply(latest);
                return;
            }
            if (frame !== undefined) return;
            frame = schedule(() => {
                frame = undefined;
                apply(latest);
            });
        },
        dispose() {
            if (frame !== undefined) cancel(frame);
            frame = undefined;
        },
    };
}

/** Where the engine cut a long output, a divider; its saved whole opens from the link. */
function omittedDivider(omitted: number, file?: string) {
    const divider = document.createElement("span");
    divider.className = "my-2 flex items-center gap-2 text-xs text-ink-faint select-none";
    const rule = () => {
        const line = document.createElement("span");
        line.className = "h-px flex-1 bg-edge";
        return line;
    };
    const label = document.createElement("span");
    label.textContent = t("drift.shell.omitted", { size: formatBytes(omitted) });
    divider.append(rule(), label);
    if (file) {
        const open = document.createElement("button");
        open.type = "button";
        open.className = "text-accent hover:underline";
        open.textContent = t("drift.shell.openFull");
        open.title = file;
        open.onclick = () => void openFile(file).catch(() => undefined);
        divider.append(open);
    }
    divider.append(rule());
    return divider;
}

function ShellOutput(props: { command: string; output: string; running: boolean; file?: string }) {
    const [copied, setCopied] = createSignal(false);
    const [renderRevision, setRenderRevision] = createSignal(0);
    let viewport!: HTMLPreElement;
    let mounted = false;
    let savedTop = 0;
    let following = true;
    /** The text node holding streamed output, so appends never disturb the styled command line. */
    let outputNode: Text | undefined;
    const stream = createShellTranscriptStream();
    const normalizer = createFrameCoalescer(
        requestAnimationFrame,
        cancelAnimationFrame,
        ({ command, output, running }: { command: string; output: string; running: boolean }) => {
            const update = stream.update(command, output, !running);
            if (!mounted) return;
            if (update.replace) {
                const segments = shellReplaceSegments(command, update.text);
                const cut = splitOmitted(segments.output);
                outputNode = document.createTextNode(cut ? cut.tail : segments.output);
                const shown: Node[] = cut
                    ? [document.createTextNode(cut.head), omittedDivider(cut.omitted, props.file), outputNode]
                    : [outputNode];
                if (segments.command === null) {
                    viewport.replaceChildren(...shown);
                } else {
                    const prompt = document.createElement("span");
                    prompt.className = "text-accent select-none";
                    prompt.textContent = "$ ";
                    const name = document.createElement("span");
                    name.className = "font-medium text-ink";
                    name.textContent = segments.command;
                    const trailing = document.createElement("span");
                    trailing.className = "text-ink-muted";
                    trailing.append(...shown);
                    viewport.replaceChildren(prompt, name, trailing);
                }
            } else if (update.text) {
                if (outputNode) outputNode.appendData(update.text);
                else viewport.append(update.text);
            }
            if (update.replace || update.text) setRenderRevision((value) => value + 1);
        },
    );
    onMount(() => {
        mounted = true;
        normalizer.push({ command: props.command, output: props.output, running: props.running }, false);
    });
    createEffect(
        on(
            () => [props.command, props.output, props.running] as const,
            ([command, output, running]) => {
                normalizer.push({ command, output, running }, running);
            },
            { defer: true },
        ),
    );
    onCleanup(() => normalizer.dispose());
    const copy = async () => {
        await navigator.clipboard.writeText(shellTranscript(props.command, props.output));
        setCopied(true);
        setTimeout(() => setCopied(false), copiedFeedbackMs);
    };
    createEffect(
        on(renderRevision, () => {
            const top = savedTop;
            const follow = following;
            queueMicrotask(() => {
                viewport.scrollTop = shellScrollTarget(top, follow, viewport.scrollHeight);
            });
        }),
    );
    return (
        <div class="group/shell relative overflow-hidden rounded-[6px] border-[0.5px] border-edge">
            <button
                title={t("drift.shell.copyOutput")}
                class="absolute top-1 right-1 z-10 flex size-6 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-focus-within/shell:opacity-100 group-hover/shell:opacity-100 hover:bg-raised hover:text-ink"
                onClick={() => void copy()}
            >
                <Show when={copied()} fallback={<IconCopy class="size-3.5" />}>
                    <IconCheck class="size-3.5" />
                </Show>
            </button>
            <pre
                ref={viewport}
                class="shell-output transcript-tool-output code-display max-h-60 overflow-x-hidden overflow-y-auto p-3 pr-10 font-mono leading-[1.5] text-ink"
                role="region"
                aria-label={t("drift.shell.output")}
                tabIndex={0}
                onScroll={(event) => {
                    savedTop = event.currentTarget.scrollTop;
                    following = shellAtBottom(
                        savedTop,
                        event.currentTarget.clientHeight,
                        event.currentTarget.scrollHeight,
                    );
                }}
            />
        </div>
    );
}

export function shellAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollHeight - clientHeight - scrollTop <= bottomSlopPx;
}

export function shellScrollTarget(savedTop: number, following: boolean, scrollHeight: number) {
    return following ? scrollHeight : savedTop;
}

export function taskBody(part: ToolPart) {
    if (part.name !== "task" && part.name !== "spawn_thread") return null;
    const input = toolInput(part) as { prompt?: string; task?: string };
    const output = part.status === "done" ? (part.output ?? "") : "";
    const result = output.match(/<task_result>\n?([\s\S]*?)\n?<\/task_result>/)?.[1] ?? output;
    // The engine tells the model how to continue the subagent; the card shows the subagent's own words.
    return { prompt: input.prompt ?? input.task ?? "", result: result.replace(/\n\n\(task_id: [^)]*\)$/, "") };
}

function GenericBody(props: { part: ToolPart }) {
    const state = () => toolDisplay(props.part);
    const output = () => (state().status === "completed" ? (state() as { output: string }).output : "");
    const showInput = () => !!toolInfo(props.part).called;
    return (
        <div class="space-y-1.5 border-l-2 border-edge pl-3">
            <Show when={showInput()}>
                <div class="font-mono text-xs break-all whitespace-pre-wrap text-ink-faint">
                    {JSON.stringify(state().input, null, 1)}
                </div>
            </Show>
            <Show when={output().trim()}>
                <div class="transcript-tool-output max-h-64 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-muted">
                    {clip(stripAnsi(output()))}
                </div>
            </Show>
        </div>
    );
}

type DiffRow = { kind: "add" | "del" | "ctx" | "gap"; line?: number; text: string };

export function parseDiff(diff: string): DiffRow[] {
    const rows: DiffRow[] = [];
    let oldLine = 0;
    let newLine = 0;
    let oldRemaining = 0;
    let newRemaining = 0;
    let inHunk = false;
    const lines = diff.split("\n");
    if (lines.at(-1) === "") lines.pop();
    for (const line of lines) {
        const hunk = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/);
        if (hunk) {
            oldLine = Number(hunk[1]);
            oldRemaining = hunk[2] === undefined ? 1 : Number(hunk[2]);
            newLine = Number(hunk[3]);
            newRemaining = hunk[4] === undefined ? 1 : Number(hunk[4]);
            inHunk = true;
            if (rows.length) rows.push({ kind: "gap", text: "" });
            continue;
        }
        if (!inHunk) continue;
        if (line.startsWith("\\")) continue;
        if (line.startsWith("+")) {
            rows.push({ kind: "add", line: newLine++, text: line.slice(1) });
            newRemaining--;
        } else if (line.startsWith("-")) {
            rows.push({ kind: "del", line: oldLine++, text: line.slice(1) });
            oldRemaining--;
        } else if (line.startsWith(" ")) {
            rows.push({ kind: "ctx", line: newLine, text: line.slice(1) });
            oldLine++;
            newLine++;
            oldRemaining--;
            newRemaining--;
        }
        if (oldRemaining <= 0 && newRemaining <= 0) inHunk = false;
    }
    return rows;
}

/**
 * Identifies one exact highlighting result: the same file, code, language and theme.
 *
 * Keying by content rather than by object identity is what keeps a diff highlighted. Every
 * transcript update rebuilds the parsed rows, and an identity check would treat that equal content
 * as new work, blanking the colours until shiki answered again. An empty key means the language is
 * still resolving, or resolved against a filename this panel no longer shows.
 */
export function diffHighlightKey(
    theme: string,
    language: { filename: string; value: string } | undefined,
    filename: string,
    code: string,
) {
    if (!language || language.filename !== filename) return "";
    return `${theme}\0${language.value}\0${code}`;
}

export function DiffPanel(props: { diff: string; filename: string; bare?: boolean }) {
    const rows = createMemo(() => parseDiff(props.diff));
    const code = createMemo(() =>
        rows()
            .map((row) => row.text)
            .join("\n"),
    );
    const [language, setLanguage] = createSignal<{ filename: string; value: string }>();
    const [highlight, setHighlight] = createSignal<{ key: string; tokens: SyntaxToken[][] }>();
    let languageRequest = 0;
    let request = 0;
    createEffect(() => {
        const filename = props.filename;
        const current = ++languageRequest;
        setLanguage(undefined);
        void resolveFileLanguage(filename)
            // A failed catalog load must degrade to plain text, not leave the panel unhighlighted forever.
            .catch(() => "text")
            .then((value) => {
                if (current === languageRequest) setLanguage({ filename, value });
            });
    });
    const highlightKey = createMemo(() => diffHighlightKey(syntaxTheme(), language(), props.filename, code()));
    createEffect(() => {
        const key = highlightKey();
        const resolved = language();
        const current = ++request;
        if (!key || !resolved) return;
        if (untrack(() => highlight()?.key) === key) return;
        void codeTokens(code(), resolved.value)
            .catch(() => [] as SyntaxToken[][])
            .then((tokens) => {
                if (current === request) setHighlight({ key, tokens });
            });
    });
    const visibleTokens = () => (highlight()?.key === highlightKey() ? (highlight()?.tokens ?? []) : []);
    return (
        <div class="diff-view overflow-hidden" classList={{ "rounded-lg border border-edge": !props.bare }}>
            <div class="transcript-tool-output max-h-80 overflow-auto py-1 font-mono leading-relaxed">
                <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
                    <For each={rows()}>
                        {(row, index) => (
                            <Show
                                when={row.kind !== "gap"}
                                fallback={<div class="my-1 border-t border-dashed border-edge" />}
                            >
                                <div
                                    class="flex border-l-2"
                                    classList={{
                                        "bg-ok/10": diffIndicator() === "background" && row.kind === "add",
                                        "bg-danger/10": diffIndicator() === "background" && row.kind === "del",
                                        "border-l-ok": diffIndicator() === "bars" && row.kind === "add",
                                        "border-l-danger": diffIndicator() === "bars" && row.kind === "del",
                                        "border-l-transparent": diffIndicator() !== "bars" || row.kind === "ctx",
                                    }}
                                >
                                    <Show when={diffLineNumbers()}>
                                        <span class="w-10 shrink-0 pr-2 text-right text-ink-faint select-none">
                                            {row.line}
                                        </span>
                                    </Show>
                                    <Show when={diffIndicator() === "symbols"}>
                                        <span
                                            class="w-4 shrink-0 text-center text-ink-faint select-none"
                                            classList={{
                                                "text-ok": row.kind === "add",
                                                "text-danger": row.kind === "del",
                                            }}
                                        >
                                            {diffSymbol(row.kind)}
                                        </span>
                                    </Show>
                                    <span
                                        class="min-w-0 flex-1 pr-3 text-ink-muted"
                                        classList={{
                                            "whitespace-pre-wrap [overflow-wrap:anywhere]": diffWordWrap(),
                                            "whitespace-pre": !diffWordWrap(),
                                        }}
                                    >
                                        <Show
                                            when={
                                                visibleTokens()[index()]?.length ? visibleTokens()[index()] : undefined
                                            }
                                            fallback={
                                                <span
                                                    classList={{
                                                        "text-ok": row.kind === "add",
                                                        "text-danger": row.kind === "del",
                                                    }}
                                                >
                                                    {row.text}
                                                </span>
                                            }
                                        >
                                            {(line) => (
                                                <For each={line()}>{(token) => <SyntaxTokenView token={token} />}</For>
                                            )}
                                        </Show>
                                    </span>
                                </div>
                            </Show>
                        )}
                    </For>
                </div>
            </div>
        </div>
    );
}

function SyntaxTokenView(props: { token: SyntaxToken }) {
    const style = () => {
        const fontStyle = props.token.fontStyle ?? 0;
        return {
            color: props.token.color,
            "font-style": fontStyle & fontStyleItalic ? "italic" : undefined,
            "font-weight": fontStyle & fontStyleBold ? "bold" : undefined,
            "text-decoration": fontStyle & fontStyleUnderline ? "underline" : undefined,
        };
    };
    return <span style={style()}>{props.token.content}</span>;
}

function diffSymbol(kind: DiffRow["kind"]) {
    if (kind === "add") return "+";
    if (kind === "del") return "-";

    return " ";
}

function PatchPanel(props: { files: PatchFile[] }) {
    return (
        <div class="space-y-2">
            <For each={props.files}>{(file) => <PatchFilePanel file={file} />}</For>
        </div>
    );
}

function PatchFilePanel(props: { file: PatchFile }) {
    const [open, setOpen] = createSignal(true);
    const status = () => {
        if (props.file.type === "add") return t("drift.file.created");
        if (props.file.type === "delete") return t("drift.file.deleted");
        if (props.file.type === "move") return t("drift.file.moved");
        return null;
    };
    return (
        <div class="overflow-hidden rounded-lg border border-edge">
            <button
                class="flex w-full items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-raised/50"
                onClick={() => setOpen(!open())}
            >
                <span class="min-w-0 flex-1 truncate font-mono text-xs text-ink-muted">
                    {props.file.relativePath ?? props.file.filePath}
                </span>
                <span class="shrink-0 font-mono text-xs">
                    <span class="text-ok">+{props.file.additions}</span>{" "}
                    <span class="text-danger">-{props.file.deletions}</span>
                </span>
                <Show when={status()}>{(label) => <span class="shrink-0 text-xs text-ink-faint">{label()}</span>}</Show>
                <Chevron open={open()} />
            </button>
            <Show when={open()}>
                <div class="border-t border-edge">
                    <DiffPanel diff={props.file.patch} filename={props.file.relativePath ?? props.file.filePath} bare />
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

function stripAnsi(value: string) {
    return value.replace(/\u001b(?:\[[0-?]*[ -/]*[@-~]|[@-_])/g, "");
}

function clip(value: string) {
    return value.length > maxInlineOutputChars ? value.slice(0, maxInlineOutputChars) + "\n..." : value;
}
