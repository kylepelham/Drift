import { createMemo, createRenderEffect, createSignal, For, Match, onMount, Show, Switch } from "solid-js";
import { groupParts, updatePartGroupSlots, type PartGroup, type PartGroupSlot } from "./message-groups";
import { collapseCompaction, compactionCollapsed, editedFilesCard } from "../state/prefs";
import { messageText, modelInfo, sessionBusy, type MessageEntry } from "../engine/store";
import { clarificationAnswer, type ClarificationAnswer } from "./clarification-answer";
import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { IconArrowUpRight, IconBranch, IconCheck, IconCopy, IconUndo } from "./icons";
import { ORCHESTRATOR_AGENT, splitOrchestratorStatus } from "../state/orchestrator";
import { selectedSession, selectSession } from "../state/selection";
import { messageModel, messageProblem } from "../engine/messages";
import { createAutoRead, ReadAloudButton } from "./reply-voice";
import { PartView, partVisible, PluginRow } from "./parts";
import { citationFileGroups } from "./citation-files";
import { EditedFilesCard } from "./edited-files-card";
import { emitMessageRendered } from "../plugins";
import { ProcessExitRow } from "./process-row";
import { agentLabel, t } from "../state/i18n";
import { TextShimmer } from "./text-shimmer";
import { ExploredGroup } from "./tool-view";
import { retryCompaction } from "./revert";
import { FilePartView } from "./file-part";
import { RefusalNotice } from "./refusal";
import { useEngine } from "../engine";
import { Markdown } from "./markdown";
import { ToolRun } from "./tool-run";
import { NoteBox } from "./note-box";
import { Chevron } from "./controls";
import {
    InstructionUpdateRow,
    ResumedRow,
    type ContinuationPart,
    type InstructionUpdatePart,
} from "./instruction-update";

import type { ContextPart, ProcessExitPart } from "../engine/parts";
import type { Message, MessageProblem } from "../engine/messages";

/**
 * `hideError`: the reply's failure is no longer news (it is being retried, or the session went on), so its error box is
 * left out; a stop's divider stays.
 */
export function MessageView(props: {
    entry: MessageEntry;
    footer?: boolean;
    groups?: PartGroup[];
    thinking?: boolean;
    spawned?: boolean;
    hideError?: boolean;
}) {
    onMount(() =>
        emitMessageRendered({
            sessionId: props.entry.info.sessionId,
            messageId: props.entry.info.id,
            role: props.entry.info.role,
        }),
    );
    const summary = () => props.entry.info.summary && collapseCompaction();
    return (
        <Show
            when={props.entry.info.role === "assistant"}
            fallback={<UserBubble entry={props.entry} thinking={props.thinking} spawned={props.spawned} />}
        >
            <Show
                when={summary()}
                fallback={
                    <AssistantFlow
                        entry={props.entry}
                        footer={props.footer}
                        groups={props.groups}
                        hideError={props.hideError}
                    />
                }
            >
                <CompactionSummary entry={props.entry} footer={props.footer} thinking={props.thinking} />
            </Show>
        </Show>
    );
}

/** A turn that paused itself says why; a plain stop reads as interrupted. */
function interruptionText(error: MessageProblem) {
    return error.text || t("drift.message.interrupted");
}

export function messageVisible(entry: MessageEntry) {
    if (entry.info.role === "user")
        return (
            !!messageText(entry) ||
            entry.parts.some((part) =>
                ["file", "compaction", "context", "process_exit", "instruction_update", "continuation"].includes(
                    part.type,
                ),
            )
        );

    const info = entry.info;
    if (info.summary && collapseCompaction()) return true;

    return entry.parts.some(partVisible) || !!messageProblem(info);
}

function CompactionSummary(props: { entry: MessageEntry; footer?: boolean; thinking?: boolean }) {
    const failed = () => !!messageProblem(props.entry.info);
    const [toggled, setOpen] = createSignal<boolean>();
    // A failed summary stays open so its error and retry are seen.
    const open = () => toggled() ?? (!compactionCollapsed() || failed());
    const label = () => {
        if (props.thinking) return t("drift.context.compacting");
        return failed() ? t("drift.message.compactionFailed") : t("drift.message.compactedSummary");
    };
    return (
        <div class="min-w-0 max-w-full">
            <button
                class="flex w-full items-center gap-3 py-1 text-xs text-ink-faint transition-colors hover:text-ink-muted"
                aria-expanded={open()}
                onClick={() => setOpen(!open())}
            >
                <div class="h-px flex-1 bg-edge" />
                <span class="flex items-center gap-1.5">
                    <Chevron open={open()} />
                    <TextShimmer text={label()} active={!!props.thinking} />
                </span>
                <div class="h-px flex-1 bg-edge" />
            </button>
            <Show when={open()}>
                <div class="mt-2 cursor-pointer border-l-2 border-edge pl-3" onClick={() => setOpen(false)}>
                    <AssistantFlow entry={props.entry} footer={props.footer} />
                </div>
            </Show>
        </div>
    );
}

/** Offered while the failed summary is still the session's last word; later turns would go with the undo. */
function CompactionRetry(props: { entry: MessageEntry }) {
    const engine = useEngine();
    const [sending, setSending] = createSignal(false);
    const session = () => props.entry.info.sessionId;
    const latest = () =>
        !sessionBusy(engine.state, session()) &&
        engine.state.transcripts[session()]?.at(-1)?.info.id === props.entry.info.id;

    const retry = async () => {
        setSending(true);
        try {
            await retryCompaction(engine, session(), props.entry.info.id);
        } finally {
            setSending(false);
        }
    };

    return (
        <Show when={latest()}>
            <button
                class="self-start rounded-md border border-edge px-2 py-1 text-xs text-ink-muted transition-colors hover:border-accent/50 hover:text-ink disabled:opacity-50"
                disabled={sending()}
                onClick={() => void retry()}
            >
                {t("drift.message.compactionRetry")}
            </button>
        </Show>
    );
}

export function compactionParts(entry: MessageEntry) {
    return entry.parts.filter((part) => part.type === "compaction");
}

/**
 * The collapsible summary row is a compaction's one marker; the prompt's divider stands in only before that row exists,
 * or when summaries are not collapsible.
 */
export function boundaryCompactions(entry: MessageEntry, collapsible: boolean, starting: boolean) {
    return collapsible && !starting ? [] : compactionParts(entry);
}

function UserBubble(props: { entry: MessageEntry; thinking?: boolean; spawned?: boolean }) {
    const engine = useEngine();
    const info = () => props.entry.info;
    const answered = createMemo(() => clarificationAnswer(props.entry));
    // A spawned thread's instruction and an orchestrator nudge fold like an answer: a label, a preview, the full text on open.
    const clarification = createMemo(
        () =>
            answered() ??
            (props.spawned ? spawnedInstruction(messageText(props.entry)) : undefined) ??
            orchestratorNudge(props.entry),
    );
    const text = () => clarification()?.text ?? messageText(props.entry);
    // Seed prompts carried into spawned threads are machine-written and keep full Markdown.
    const generated = () => props.entry.parts.some((part) => part.type === "nudge");
    const files = () => props.entry.parts.filter((part) => part.type === "file");
    const plugins = () => props.entry.parts.filter((part): part is ContextPart => part.type === "context");
    const exits = () => props.entry.parts.filter((part): part is ProcessExitPart => part.type === "process_exit");
    const updates = () =>
        props.entry.parts.filter((part): part is InstructionUpdatePart => part.type === "instruction_update");
    const resumed = () => props.entry.parts.filter((part): part is ContinuationPart => part.type === "continuation");
    const compactions = () => boundaryCompactions(props.entry, collapseCompaction(), !!props.thinking);

    // Shift keeps every file as it is: only the conversation goes back.
    const revert = (keepFiles: boolean) => {
        const restored = draftFromMessage(props.entry);
        if (clarification()) restored.text = text();
        void engine.actions.revert(info().sessionId, info().id, keepFiles).then((reverted) => {
            if (reverted) setComposerDraft(composerScope(info().sessionId), restored);
        });
    };

    return (
        <>
            <For each={plugins()}>{(part) => <PluginRow part={part} end />}</For>
            <For each={exits()}>{(part) => <ProcessExitRow part={part} />}</For>
            <For each={updates()}>{(part) => <InstructionUpdateRow part={part} />}</For>
            <For each={resumed()}>{(part) => <ResumedRow part={part} />}</For>
            <Show
                when={clarification()}
                fallback={
                    <>
                        <Show when={text() || files().length > 0}>
                            <div class="group flex flex-col items-end gap-1.5">
                                <Show when={files().length > 0}>
                                    <div class="flex max-w-[85%] flex-wrap justify-end gap-1.5">
                                        <For each={files()}>
                                            {(file) => (
                                                <FilePartView
                                                    part={file}
                                                    directory={engine.state.sessions[info().sessionId]?.directory}
                                                />
                                            )}
                                        </For>
                                    </div>
                                </Show>
                                <Show when={text()}>
                                    <div class="max-w-[85%] rounded-lg border border-edge bg-surface px-3 py-1.5">
                                        <Show
                                            when={!generated() && largeUserText(text())}
                                            fallback={
                                                <Markdown
                                                    text={text()}
                                                    directory={engine.state.sessions[info().sessionId]?.directory}
                                                    fileGroups={() =>
                                                        citationFileGroups(engine.state, info().sessionId, info().id)
                                                    }
                                                    done
                                                    humanAuthored={!generated()}
                                                />
                                            }
                                        >
                                            <pre class="user-paste">{text()}</pre>
                                        </Show>
                                    </div>
                                </Show>
                                <UserActions entry={props.entry} onRevert={revert} copy={text} />
                            </div>
                        </Show>
                        <For each={compactions()}>{(part) => <PartView part={part} thinking={props.thinking} />}</For>
                    </>
                }
            >
                {(answer) => (
                    <div class="group flex min-w-0 flex-col items-end gap-1.5">
                        <NoteBox icon={noteIcon(answer())} label={noteLabel(answer())} preview={answer().preview}>
                            <Show
                                when={answer().items.length}
                                fallback={<div class="whitespace-pre-wrap break-words">{answer().text}</div>}
                            >
                                <For each={answer().items}>
                                    {(item) => (
                                        <div class="space-y-1">
                                            <div class="whitespace-pre-wrap break-words text-xs text-ink-faint">
                                                {item.question}
                                            </div>
                                            <div
                                                data-find-ignore={item.answers.length ? undefined : ""}
                                                class="whitespace-pre-wrap break-words text-ink"
                                            >
                                                {item.answers.length
                                                    ? item.answers.join(", ")
                                                    : t("drift.question.unanswered")}
                                            </div>
                                        </div>
                                    )}
                                </For>
                            </Show>
                        </NoteBox>
                        <UserActions entry={props.entry} onRevert={revert} copy={text} />
                    </div>
                )}
            </Show>
        </>
    );
}

/** Under every user message: who sent it to what and when, Undo back to it, and Copy. */
function UserActions(props: { entry: MessageEntry; onRevert: (keepFiles: boolean) => void; copy: () => string }) {
    const engine = useEngine();
    const info = () => props.entry.info;
    const model = () => modelInfo(engine.state, messageModel(info()))?.name ?? info().model?.model ?? "";
    const time = () => new Date(info().createdAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

    return (
        <div class="message-actions touch-reveal flex items-center gap-2 text-[length:calc(0.7rem*var(--ui-text-scale,1))] text-ink-faint opacity-0 transition-opacity select-none group-focus-within:opacity-100 group-hover:opacity-100">
            <span>
                {agentLabel(info().agent ?? "build")} · {model()} · {time()}
            </span>
            <button
                title={`${t("drift.message.revertHere")}
${t("drift.message.revertKeepFiles")}`}
                class="rounded p-0.5 hover:bg-raised hover:text-ink"
                onClick={(event) => props.onRevert(event.shiftKey)}
            >
                <IconUndo class="size-3.5" />
            </button>
            <button
                title={t("drift.message.copy")}
                class="rounded p-0.5 hover:bg-raised hover:text-ink"
                onClick={() => void navigator.clipboard.writeText(props.copy())}
            >
                <IconCopy class="size-3.5" />
            </button>
        </div>
    );
}

function noteIcon(answer: ClarificationAnswer) {
    if (answer.spawned) return <IconBranch class="size-3.5" />;
    if (answer.nudge) return <IconArrowUpRight class="size-3.5" />;

    return <IconCheck class="size-3.5" />;
}

function noteLabel(answer: ClarificationAnswer) {
    if (answer.spawned) return t("drift.chat.spawned.instruction");
    if (answer.nudge) return t("drift.orchestrator.nudge");

    return t("drift.question.answered");
}

/** A prompt made only of the engine's nudges, which keep an orchestrator working; not the user's words. */
function orchestratorNudge(entry: MessageEntry): ClarificationAnswer | undefined {
    if (entry.info.role !== "user" || !entry.parts.length) return;
    if (!entry.parts.every((part) => part.type === "nudge")) return;

    const text = messageText(entry);
    return { text, preview: text.replace(/\s+/g, " ").trim(), items: [], nudge: true };
}

function spawnedInstruction(text: string): ClarificationAnswer {
    return { text, preview: text.replace(/\s+/g, " ").trim(), items: [], spawned: true };
}

export function largeUserText(text: string) {
    return text.length >= 2000 || text.split("\n", 41).length > 40;
}

function AssistantFlow(props: { entry: MessageEntry; footer?: boolean; groups?: PartGroup[]; hideError?: boolean }) {
    const engine = useEngine();
    const info = () => props.entry.info;
    const problem = () => messageProblem(info());
    const slots = new Map<string, PartGroupSlot>();
    const [groups, setGroups] = createSignal<PartGroupSlot[]>([]);
    createRenderEffect(() => setGroups(updatePartGroupSlots(props.groups ?? groupParts(props.entry.parts), slots)));

    const visible = () => groups().length > 0 || !!problem() || (!!props.footer && !!info().finishedAt);
    createAutoRead(
        () => props.entry,
        () => !!props.footer,
    );

    /**
     * A new conversation with this conversation's history through this reply, opened only if the user is still here.
     */
    const forkHere = async () => {
        const source = info().sessionId;
        const forked = await engine.actions.fork(source, info().id);
        if (forked && selectedSession() === source) selectSession(forked.id);
    };

    const liveTextPartID = () => {
        if (info().finishedAt || !sessionBusy(engine.state, info().sessionId)) return undefined;
        return [...props.entry.parts].reverse().find((part) => part.type === "text")?.id;
    };

    // A step that ends in tool calls finishes while the turn goes on; only the turn's last one is its end.
    const turnOver = () =>
        !sessionBusy(engine.state, info().sessionId) ||
        engine.state.transcripts[info().sessionId]?.at(-1)?.info.id !== info().id;

    return (
        <Show when={visible()}>
            <div class="group flex min-w-0 max-w-full flex-col gap-3">
                <For each={groups()}>
                    {(group) => (
                        <Switch>
                            <Match when={"explored" in group.value && group.value}>
                                {(explored) => <ExploredGroup parts={explored().explored} />}
                            </Match>
                            <Match when={"run" in group.value && group.value}>
                                {(run) => <ToolRun parts={run().run} />}
                            </Match>
                            <Match when={"part" in group.value && group.value}>
                                {(single) => (
                                    <PartView
                                        part={single().part}
                                        revision={group.revision?.()}
                                        responseID={`${info().id}:${single().part.id}`}
                                        live={single().part.id === liveTextPartID()}
                                        orchestrated={info().agent === ORCHESTRATOR_AGENT}
                                    />
                                )}
                            </Match>
                        </Switch>
                    )}
                </For>
                <Show when={!(props.hideError && !problem()?.interrupted) && problem()}>
                    {(error) => (
                        <Show
                            when={!error().interrupted}
                            fallback={
                                <div class="flex items-center gap-3 py-1 text-xs text-ink-faint" role="status">
                                    <div class="h-px flex-1 bg-edge" />
                                    {interruptionText(error())}
                                    <div class="h-px flex-1 bg-edge" />
                                </div>
                            }
                        >
                            <Show
                                when={info().ending === "refused"}
                                fallback={
                                    <div
                                        class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm break-words text-danger"
                                        role="alert"
                                    >
                                        {error().text}
                                    </div>
                                }
                            >
                                <RefusalNotice entry={props.entry} text={error().text} />
                            </Show>
                            <Show when={info().summary}>
                                <CompactionRetry entry={props.entry} />
                            </Show>
                        </Show>
                    )}
                </Show>
                <Show when={props.footer && info().finishedAt && turnOver() && editedFilesCard()}>
                    <EditedFilesCard entry={props.entry} />
                </Show>
                <Show when={props.footer && info().finishedAt}>
                    <div class="message-actions touch-reveal flex items-center gap-3 text-[length:calc(0.7rem*var(--ui-text-scale,1))] text-ink-faint opacity-0 transition-opacity duration-200 select-none group-hover:opacity-100">
                        <span>{info().model?.model ?? ""}</span>
                        <span>{formatTokens(info())}</span>
                        <Show when={tokensPerSecond(props.entry)}>
                            {(rate) => <span>{t("drift.message.tokensPerSecond", { rate: rate() })}</span>}
                        </Show>
                        <Show when={info().cost > 0}>
                            <span>${info().cost.toFixed(3)}</span>
                        </Show>
                        <span>{formatDuration(info().finishedAt! - info().createdAt)}</span>
                        <button
                            title={t("drift.message.copyResponse")}
                            class="rounded p-0.5 hover:bg-raised hover:text-ink"
                            onClick={() =>
                                void navigator.clipboard.writeText(
                                    splitOrchestratorStatus(messageText(props.entry)).prose,
                                )
                            }
                        >
                            <IconCopy class="size-3.5" />
                        </button>
                        <ReadAloudButton entry={props.entry} />
                        <button
                            title={t("drift.message.forkHere")}
                            class="rounded p-0.5 hover:bg-raised hover:text-ink"
                            onClick={() => void forkHere()}
                        >
                            <IconBranch class="size-3.5" />
                        </button>
                    </div>
                </Show>
            </div>
        </Show>
    );
}

function formatDuration(ms: number) {
    const seconds = Math.max(1, Math.round(ms / 1000));
    if (seconds < 60) return t("drift.message.duration.seconds", { seconds });

    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return t("drift.message.duration.minutes", { minutes, seconds: seconds % 60 });

    return t("drift.message.duration.hours", { hours: Math.floor(minutes / 60), minutes: minutes % 60 });
}

/** Measured by the engine's response clock; old replies have no reliable generation duration. */
export function generationMs(entry: MessageEntry) {
    return entry.info.generationMs ?? 0;
}

/** Output tokens over the measured generation time; null when there is no usable measurement. */
export function tokensPerSecond(entry: MessageEntry) {
    const elapsed = generationMs(entry);
    const tokens = entry.info.usage.output;

    const measured = Number.isFinite(elapsed) && elapsed > 0;
    const produced = Number.isFinite(tokens) && tokens > 0;
    if (!measured || !produced) return null;

    return (tokens / (elapsed / 1000)).toFixed(1);
}

function formatTokens(info: Message) {
    const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);
    return t("drift.message.tokenCounts", {
        input: compact(info.usage.input),
        output: compact(info.usage.output),
    });
}
