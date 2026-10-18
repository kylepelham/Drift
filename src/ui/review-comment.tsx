import { IconArrowUp, IconPlus, IconSquarePen, IconTrash } from "./icons";
import { createSignal, onMount } from "solid-js";
import { t } from "../state/i18n";

import type { ReviewComment } from "../state/composer";

/** Where a note sits: under its line, as wide as the diff's view however far the lines scroll sideways. */
const boxClass = "sticky left-0 w-[100cqw] px-3 py-1.5 font-sans";

function lineLabel(side: ReviewComment["side"], line: number) {
    return t(side === "del" ? "drift.review.comment.removedLine" : "drift.review.comment.line", { line });
}

/** A note being written or edited: Enter saves, Shift+Enter breaks the line, Escape gives up. */
export function CommentEditor(props: {
    side: ReviewComment["side"];
    line: number;
    initial?: string;
    onSave: (text: string) => void;
    onCancel: () => void;
}) {
    const [text, setText] = createSignal(props.initial ?? "");
    let area!: HTMLTextAreaElement;
    const save = () => {
        if (text().trim()) props.onSave(text().trim());
    };
    const fit = () => {
        area.style.height = "auto";
        area.style.height = `${area.scrollHeight}px`;
    };
    onMount(() => {
        fit();
        area.focus();
        area.setSelectionRange(area.value.length, area.value.length);
    });

    return (
        <div class={boxClass}>
            <div class="rounded-lg border border-accent/70 bg-surface px-3 py-2">
                <div class="text-[0.7rem] text-ink-faint">{lineLabel(props.side, props.line)}</div>
                <div class="flex items-end gap-2">
                    <textarea
                        ref={area}
                        rows={1}
                        class="mt-1 min-w-0 flex-1 resize-none bg-transparent text-[0.82rem] text-ink outline-none placeholder:text-ink-faint"
                        placeholder={t("drift.review.comment.placeholder")}
                        value={text()}
                        onInput={(event) => {
                            setText(event.currentTarget.value);
                            fit();
                        }}
                        onKeyDown={(event) => {
                            if (event.key === "Escape") {
                                event.preventDefault();
                                props.onCancel();
                            } else if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
                                event.preventDefault();
                                save();
                            }
                        }}
                        onBlur={() => !text().trim() && props.onCancel()}
                    />
                    <button
                        title={t("drift.review.comment.save")}
                        aria-label={t("drift.review.comment.save")}
                        class="flex size-6 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors enabled:hover:bg-raised enabled:hover:text-ink disabled:opacity-40"
                        disabled={!text().trim()}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={save}
                    >
                        <IconArrowUp class="size-3.5" />
                    </button>
                </div>
            </div>
        </div>
    );
}

/** A saved note: what was written, with Edit and Delete; deleting it here removes it from the composer too. */
export function SavedComment(props: { comment: ReviewComment; onEdit: () => void; onRemove: () => void }) {
    return (
        <div class={boxClass}>
            <div class="group/note flex items-start gap-2 rounded-lg border border-edge bg-surface px-3 py-2">
                <div class="min-w-0 flex-1">
                    <div class="text-[0.7rem] text-ink-faint">{lineLabel(props.comment.side, props.comment.line)}</div>
                    <div class="mt-1 text-[0.82rem] whitespace-pre-wrap text-ink [overflow-wrap:anywhere]">
                        {props.comment.text}
                    </div>
                </div>
                <div class="touch-reveal flex shrink-0 gap-0.5 opacity-0 transition-opacity group-hover/note:opacity-100 focus-within:opacity-100">
                    <button
                        title={t("drift.review.comment.edit")}
                        aria-label={t("drift.review.comment.edit")}
                        class="flex size-6 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        onClick={() => props.onEdit()}
                    >
                        <IconSquarePen class="size-3.5" />
                    </button>
                    <button
                        title={t("drift.review.comment.remove")}
                        aria-label={t("drift.review.comment.remove")}
                        class="flex size-6 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-danger"
                        onClick={() => props.onRemove()}
                    >
                        <IconTrash class="size-3.5" />
                    </button>
                </div>
            </div>
        </div>
    );
}

/** The + beside a line, shown while the pointer is over it. */
export function CommentButton(props: { onClick: () => void }) {
    return (
        <button
            title={t("drift.review.comment.add")}
            aria-label={t("drift.review.comment.add")}
            class="touch-reveal absolute top-1/2 left-0.5 z-[1] flex size-4 -translate-y-1/2 items-center justify-center rounded bg-accent text-accent-ink opacity-0 transition-opacity group-hover/line:opacity-100 focus-visible:opacity-100"
            onClick={() => props.onClick()}
        >
            <IconPlus class="size-3" />
        </button>
    );
}
