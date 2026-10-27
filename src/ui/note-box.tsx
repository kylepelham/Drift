import { Show, type JSX } from "solid-js";
import { Chevron } from "./controls";

const summaryClass = "flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-ink-muted";

/** A folded note in the transcript, as an answered question shows: an icon, a label and a preview; open for the rest.
 * Without a body it is the same line, with nothing to open. */
export function NoteBox(props: { icon: JSX.Element; label: JSX.Element; preview?: string; children?: JSX.Element }) {
    const line = () => (
        <>
            <span class="flex size-3.5 shrink-0 items-center justify-center text-ink-faint">{props.icon}</span>
            <span class="shrink-0">{props.label}</span>
            <Show when={props.preview}>
                <span class="min-w-0 truncate text-ink" title={props.preview}>
                    {props.preview}
                </span>
            </Show>
        </>
    );

    return (
        <Show
            when={props.children}
            fallback={
                <div data-find-ignore class={`${summaryClass} max-w-[min(85%,28rem)] text-xs`}>
                    {line()}
                </div>
            }
        >
            <details class="group/note min-w-0 max-w-[min(85%,28rem)] text-xs">
                <summary
                    data-find-ignore
                    class={`${summaryClass} cursor-pointer list-none hover:bg-raised/40 [&::-webkit-details-marker]:hidden`}
                >
                    {line()}
                    <span class="shrink-0 transition-transform group-open/note:rotate-90">
                        <Chevron open={false} />
                    </span>
                </summary>
                <div class="mt-1 mb-2 space-y-3 border-l border-edge px-3 py-1 text-sm">{props.children}</div>
            </details>
        </Show>
    );
}
