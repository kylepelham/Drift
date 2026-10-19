import { createEffect, createSignal, For, on, Show } from "solid-js";
import { IconArrowUp, IconSquarePen, IconTrash } from "./icons";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { QueuedPrompt } from "../engine/store";

const action =
    "flex size-6 items-center justify-center rounded text-ink-faint transition-colors hover:bg-raised hover:text-ink";

/** A queued prompt's text as the row shows it, with any files it carries counted. */
export function queuedText(item: QueuedPrompt) {
    const text = item.prompt.parts
        .flatMap((part) => (part.type === "text" ? [part.text] : []))
        .join("\n")
        .trim();
    const files = item.prompt.parts.filter((part) => part.type === "file").length;
    const counted = files ? t("drift.queue.files", { count: String(files) }) : "";

    return [text, counted].filter(Boolean).join(" ");
}

/** Prompts waiting for the running turn to end, above the composer: edit, discard, or send now, which steers. */
export function ComposerQueue(props: { sessionId: string }) {
    const engine = useEngine();
    createEffect(
        on(
            () => props.sessionId,
            (id) => void engine.actions.loadQueue(id).catch(() => undefined),
        ),
    );
    const queued = () => engine.state.queued[props.sessionId] ?? [];

    return (
        <Show when={queued().length > 0}>
            <div class="mx-auto mb-2 max-w-3xl space-y-1">
                <For each={queued()}>{(item) => <QueuedRow item={item} />}</For>
            </div>
        </Show>
    );
}

function QueuedRow(props: { item: QueuedPrompt }) {
    const engine = useEngine();
    const [editing, setEditing] = createSignal(false);
    const [draft, setDraft] = createSignal("");

    async function attempt(run: () => Promise<unknown>) {
        try {
            await run();
        } catch (cause) {
            engine.actions.notice({
                message: cause instanceof Error ? cause.message : String(cause),
                variant: "error",
            });
        }
    }
    const startEditing = () => {
        setDraft(queuedText(props.item));
        setEditing(true);
    };
    const save = async () => {
        const [item, text] = [props.item, draft()];
        await attempt(() => engine.actions.editQueued(item, text));
        setEditing(false);
    };

    return (
        <div class="rounded-lg border border-edge bg-surface px-3 py-2 text-xs">
            <Show
                when={editing()}
                fallback={
                    <div class="flex items-center gap-2">
                        <span class="shrink-0 text-ink-faint">{t("drift.queue.label")}</span>
                        <span class="min-w-0 flex-1 truncate text-ink-muted">{queuedText(props.item)}</span>
                        <button
                            class={action}
                            title={t("drift.queue.edit")}
                            aria-label={t("drift.queue.edit")}
                            onClick={startEditing}
                        >
                            <IconSquarePen class="size-3.5" />
                        </button>
                        <button
                            class={action}
                            title={t("drift.queue.sendNow")}
                            aria-label={t("drift.queue.sendNow")}
                            onClick={() => void attempt(() => engine.actions.sendQueuedNow(props.item.id))}
                        >
                            <IconArrowUp class="size-3.5" />
                        </button>
                        <button
                            class={action}
                            title={t("drift.queue.discard")}
                            aria-label={t("drift.queue.discard")}
                            onClick={() => void attempt(() => engine.actions.discardQueued(props.item.id))}
                        >
                            <IconTrash class="size-3.5" />
                        </button>
                    </div>
                }
            >
                <textarea
                    class="w-full resize-none rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-ink outline-none focus:border-edge-strong"
                    rows={3}
                    aria-label={t("drift.queue.edit")}
                    value={draft()}
                    onInput={(event) => setDraft(event.currentTarget.value)}
                />
                <div class="mt-1.5 flex justify-end gap-2">
                    <button
                        class="rounded-md px-2.5 py-1 text-ink-muted hover:text-ink"
                        onClick={() => setEditing(false)}
                    >
                        {t("drift.queue.cancel")}
                    </button>
                    <button class="rounded-md bg-accent px-2.5 py-1 text-white" onClick={() => void save()}>
                        {t("drift.queue.save")}
                    </button>
                </div>
            </Show>
            <Show when={props.item.error}>
                <div class="mt-1 text-danger">{t("drift.queue.failed", { reason: props.item.error ?? "" })}</div>
            </Show>
        </div>
    );
}
