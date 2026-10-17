import { editedFiles, turnOf, turnTasks, type EditedFile, type TurnWorker } from "./edited-files";
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { delegatedChildId } from "./tool-delegation";
import { openReview } from "../state/panels";
import { toolInput } from "../engine/parts";
import { FileIcon } from "./file-icon";
import { useEngine } from "../engine";
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

function FileRow(props: { file: EditedFile; onOpen: () => void }) {
    return (
        <button
            class="flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left transition-colors hover:bg-raised/45"
            title={props.file.path}
            onClick={() => props.onOpen()}
        >
            <FileIcon path={props.file.path} class="size-4" />
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
        </button>
    );
}

/** Files listed before "See all" opens the rest in the review panel. */
const shownFiles = 3;

/** At the end of a turn: the files it and its subagents changed, each opening the turn's review, and Undo for the whole turn. */
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

    const review = (focus?: string) => {
        const prompt = turn().prompt;
        if (prompt) openReview({ source: { kind: "turn", sessionId: sessionId(), messageId: prompt.info.id }, focus });
    };

    return (
        <Show when={files().length}>
            {(count) => (
                <div class="edited-files rounded-lg border border-edge/70 p-1.5 text-sm">
                    <div class="flex min-w-0 items-center gap-2 px-2 py-1">
                        <span class="min-w-0 truncate text-ink-muted">
                            {t(count() === 1 ? "drift.editedFiles.title.one" : "drift.editedFiles.title.other", {
                                count: count(),
                            })}
                        </span>
                        <Counts additions={total().additions} deletions={total().deletions} />
                        <span class="flex-1" />
                        <Show when={turn().prompt}>
                            <button
                                class="shrink-0 rounded px-1.5 py-0.5 text-xs transition-colors hover:bg-raised"
                                classList={{
                                    "text-danger": confirming(),
                                    "text-ink-faint hover:text-ink": !confirming(),
                                }}
                                title={t("drift.editedFiles.undoHint")}
                                onClick={() => void undo()}
                            >
                                {confirming() ? t("drift.editedFiles.confirm") : t("drift.editedFiles.undo")}
                            </button>
                        </Show>
                    </div>
                    <For each={files().slice(0, shownFiles)}>
                        {(file) => <FileRow file={file} onOpen={() => review(file.path)} />}
                    </For>
                    <Show when={count() > shownFiles}>
                        <button
                            class="w-full rounded-md px-2 py-1 text-left text-xs text-ink-faint transition-colors hover:bg-raised/45 hover:text-ink"
                            onClick={() => review()}
                        >
                            {t("drift.editedFiles.seeAll", { count: count() })}
                        </button>
                    </Show>
                </div>
            )}
        </Show>
    );
}
