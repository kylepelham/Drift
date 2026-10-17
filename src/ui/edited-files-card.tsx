import { editedFiles, turnOf, turnTasks, type EditedFile, type TurnWorker } from "./edited-files";
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { delegatedChildId } from "./tool-delegation";
import { toolInput } from "../engine/parts";
import { DiffPanel } from "./diff-panel";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { IconUndo } from "./icons";
import { t } from "../state/i18n";

import type { MessageEntry } from "../engine/store";

/** How long Undo waits for its confirming click before it goes back to asking. */
const confirmMs = 4000;

function Counts(props: { additions: number; deletions: number }) {
    return (
        <span class="shrink-0 font-mono text-xs">
            <span class="text-ok">+{props.additions}</span> <span class="text-danger">-{props.deletions}</span>
        </span>
    );
}

function FileRow(props: { file: EditedFile }) {
    const [open, setOpen] = createSignal(false);

    return (
        <div class="border-t border-edge/60">
            <button
                class="flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-raised/35"
                aria-expanded={open()}
                title={props.file.path}
                onClick={() => setOpen(!open())}
            >
                <span class="min-w-0 truncate text-ink-muted">{props.file.name}</span>
                <Show when={props.file.by}>
                    {(by) => (
                        <span class="min-w-0 truncate text-xs text-ink-faint">
                            {t("drift.editedFiles.by", { name: by() })}
                        </span>
                    )}
                </Show>
                <span class="flex-1" />
                <Counts additions={props.file.additions} deletions={props.file.deletions} />
                <span class="flex shrink-0 text-ink-faint">
                    <Chevron open={open()} />
                </span>
            </button>
            <Show when={open()}>
                <div class="space-y-2 px-3 pb-3">
                    <For each={props.file.edits}>
                        {(edit) => <DiffPanel diff={edit.patch} filename={edit.filename} />}
                    </For>
                </div>
            </Show>
        </div>
    );
}

/** At the end of a turn: every file it and its subagents changed, with their edits and Undo for the whole turn. */
export function EditedFilesCard(props: { entry: MessageEntry }) {
    const engine = useEngine();
    const sessionId = () => props.entry.info.sessionId;
    const turn = createMemo(() => turnOf(engine.state.transcripts[sessionId()] ?? [], props.entry.info.id));
    const children = createMemo(() =>
        turnTasks(turn().replies)
            .filter((part) => part.status === "done" || part.status === "error")
            .map((part) => ({ part, id: delegatedChildId(engine.state, part) }))
            .filter((child): child is { part: (typeof child)["part"]; id: string } => !!child.id),
    );

    // A subagent's edits are in its own transcript, loaded only when a card that needs it is on screen.
    createEffect(() => {
        for (const child of children()) if (!engine.state.loaded[child.id]) void engine.actions.openSession(child.id);
    });

    const files = createMemo(() => {
        const workers: TurnWorker[] = children().map(({ part, id }) => {
            const description = toolInput(part).description;
            return {
                name: typeof description === "string" ? description : (engine.state.sessions[id]?.title ?? ""),
                entries: engine.state.transcripts[id] ?? [],
            };
        });

        return editedFiles(turn().replies, workers);
    });
    const total = () =>
        files().reduce(
            (sum, file) => ({ additions: sum.additions + file.additions, deletions: sum.deletions + file.deletions }),
            { additions: 0, deletions: 0 },
        );

    const [confirming, setConfirming] = createSignal(false);
    let reset: ReturnType<typeof setTimeout> | undefined;
    onCleanup(() => clearTimeout(reset));

    const undo = async () => {
        const prompt = turn().prompt;
        if (!prompt) return;
        if (!confirming()) {
            setConfirming(true);
            clearTimeout(reset);
            reset = setTimeout(() => setConfirming(false), confirmMs);
            return;
        }

        clearTimeout(reset);
        setConfirming(false);
        const restored = draftFromMessage(prompt);
        if (await engine.actions.revert(sessionId(), prompt.info.id))
            setComposerDraft(composerScope(sessionId()), restored);
    };

    return (
        <Show when={files().length}>
            {(count) => (
                <div class="edited-files overflow-hidden rounded-lg border border-edge/70 text-sm">
                    <div class="flex min-w-0 items-center gap-2 px-3 py-2">
                        <span class="min-w-0 truncate text-ink-muted">
                            {t(count() === 1 ? "drift.editedFiles.title.one" : "drift.editedFiles.title.other", {
                                count: count(),
                            })}
                        </span>
                        <span class="flex-1" />
                        <Show when={turn().prompt}>
                            <button
                                class="flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-xs transition-colors hover:bg-raised"
                                classList={{
                                    "text-danger": confirming(),
                                    "text-ink-faint hover:text-ink": !confirming(),
                                }}
                                title={t("drift.editedFiles.undoHint")}
                                onClick={() => void undo()}
                            >
                                <IconUndo class="size-3" />
                                {confirming() ? t("drift.editedFiles.confirm") : t("drift.editedFiles.undo")}
                            </button>
                        </Show>
                        <Counts additions={total().additions} deletions={total().deletions} />
                    </div>
                    <For each={files()}>{(file) => <FileRow file={file} />}</For>
                </div>
            )}
        </Show>
    );
}
