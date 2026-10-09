import { IconChevronLeft, IconChevronRight } from "./icons";
import { Picker } from "./picker";
import { t } from "../state/i18n";
import { Show } from "solid-js";

import type { JSX } from "solid-js";

/** One waiting request as the stack lists it. */
export type RequestStackItem = {
    id: string;
    title: string;
    thread: string;
    blocking: boolean;
};

/** Requests waiting behind the card on show, and how to bring another one forward. */
export type RequestStack = {
    items: RequestStackItem[];
    current: string;
    onSelect: (id: string) => void;
};

/** The ghost cards drawn behind the front one: one per extra request, at most two. */
export function stackDepth(stack: RequestStack | undefined) {
    const count = stack?.items.length ?? 0;

    return Math.min(Math.max(count - 1, 0), 2);
}

/** The neighbouring request in the stack, or nothing at either end. */
export function neighbour(stack: RequestStack, offset: -1 | 1) {
    const index = stack.items.findIndex((item) => item.id === stack.current);

    return stack.items[index + offset]?.id;
}

/** Wraps a card so the requests behind it show as card edges above it. */
export function RequestStackLayers(props: { stack?: RequestStack; children: JSX.Element }) {
    const depth = () => stackDepth(props.stack);

    return (
        <div class="relative" classList={{ "pt-1.5": depth() === 1, "pt-3": depth() === 2 }}>
            <Show when={depth() === 2}>
                <div
                    aria-hidden="true"
                    class="pointer-events-none absolute inset-x-6 top-0 h-8 rounded-t-xl border border-b-0 border-edge bg-surface opacity-50"
                />
            </Show>
            <Show when={depth() >= 1}>
                <div
                    aria-hidden="true"
                    class="pointer-events-none absolute inset-x-3 h-8 rounded-t-xl border border-b-0 border-edge bg-surface opacity-80"
                    classList={{ "top-0": depth() === 1, "top-1.5": depth() === 2 }}
                />
            </Show>
            {props.children}
        </div>
    );
}

/** The strip at the top of a card that steps through the waiting requests or picks one. */
export function RequestStackStrip(props: { stack?: RequestStack }) {
    return (
        <Show when={props.stack && props.stack.items.length > 1 ? props.stack : undefined}>
            {(stack) => {
                const position = () => stack().items.findIndex((item) => item.id === stack().current) + 1;
                const previous = () => neighbour(stack(), -1);
                const next = () => neighbour(stack(), 1);

                // The picker's line under each request names its thread, and says when a turn waits on it.
                const items = () =>
                    stack().items.map((item) => {
                        const waiting = item.blocking ? `${t("drift.question.blocking")} · ` : "";

                        return { id: item.id, label: item.title, detail: `${waiting}${item.thread}` };
                    });

                return (
                    <div class="flex min-w-0 items-center gap-1 border-b border-edge bg-raised/40 px-2 py-1">
                        <StepButton
                            label={t("drift.question.stack.previous")}
                            target={previous()}
                            onSelect={stack().onSelect}
                        >
                            <IconChevronLeft class="size-3.5" />
                        </StepButton>
                        <span class="shrink-0 px-1 text-xs tabular-nums text-ink-muted">
                            {t("drift.question.stack.position", { current: position(), total: stack().items.length })}
                        </span>
                        <StepButton label={t("drift.question.stack.next")} target={next()} onSelect={stack().onSelect}>
                            <IconChevronRight class="size-3.5" />
                        </StepButton>
                        <div class="ml-auto min-w-0">
                            <Picker
                                label={t("drift.question.stack.pick")}
                                items={items()}
                                selected={stack().current}
                                floating
                                chevronAtEnd
                                placement="below"
                                width="22rem"
                                onPick={stack().onSelect}
                            />
                        </div>
                    </div>
                );
            }}
        </Show>
    );
}

function StepButton(props: {
    label: string;
    target: string | undefined;
    onSelect: (id: string) => void;
    children: JSX.Element;
}) {
    return (
        <button
            class="flex size-6 shrink-0 items-center justify-center rounded text-ink-muted transition-colors hover:bg-raised hover:text-ink disabled:pointer-events-none disabled:opacity-30"
            aria-label={props.label}
            title={props.label}
            disabled={!props.target}
            onClick={() => props.target && props.onSelect(props.target)}
        >
            {props.children}
        </button>
    );
}
