import { hasPartRenderer, hasToolRenderer, PluginPartView, PluginToolView } from "../plugins";
import { splitOrchestratorStatus, type OrchestratorStatus } from "../state/orchestrator";
import { InstructionUpdateRow, type InstructionUpdatePart } from "./instruction-update";
import { createMemo, createSignal, Match, Show, Switch, type JSX } from "solid-js";
import { IconArrowUpRight, IconCheck, IconInfo, IconPlug } from "./icons";
import { openToolContextMenu } from "./tool-context-menu";
import { citationFileGroups } from "./citation-files";
import { promptPartText } from "../engine/parts";
import { showReasoning } from "../state/prefs";
import { TextShimmer } from "./text-shimmer";
import { FilePartView } from "./file-part";
import { Dynamic } from "solid-js/web";
import { ToolView } from "./tool-view";
import { Markdown } from "./markdown";
import { useEngine } from "../engine";
import { NoteBox } from "./note-box";
import { Chevron } from "./controls";
import { t } from "../state/i18n";

import type { FilePart, Part, ContextPart, ReasoningPart, ToolPart } from "../engine/parts";

const hiddenTools = new Set(["todowrite", "todoread"]);

/**
 * `orchestrated`: the orchestrator wrote the reply, so its status block shows as a row; from any other agent it is only
 * hidden.
 */
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
            <Match when={props.part.type === "instruction_update" && props.part}>
                {(part) => <InstructionUpdateRow part={part() as InstructionUpdatePart} />}
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

const statusIcons = {
    working: IconArrowUpRight,
    done: IconCheck,
    blocked: IconInfo,
} as const;

/** The orchestrator's end-of-reply status, folded like an answer rather than the JSON it wrote; on the user's
 * side, since while it says Working the engine prompts again on the user's behalf. */
function OrchestratorStatusRow(props: { status: OrchestratorStatus }) {
    return (
        <div class="flex min-w-0 justify-end">
            <NoteBox
                icon={<Dynamic component={statusIcons[props.status.state]} class="size-3.5" />}
                label={t(statusLabels[props.status.state])}
                preview={props.status.headline}
            >
                {props.status.headline && (
                    <div class="whitespace-pre-wrap break-words text-ink">{props.status.headline}</div>
                )}
            </NoteBox>
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
        case "instruction_update":
            return true;
        default:
            return false;
    }
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
