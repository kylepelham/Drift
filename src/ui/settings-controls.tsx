import { createSignal, Show } from "solid-js";
import { Chevron } from "./controls";

import type { JSX } from "solid-js";

/**
 * Layout primitives shared by the settings sections. They live here rather than in settings.tsx so a
 * section can be split into its own file without importing back into the module that renders it.
 */

/** A titled block of rows, with an optional control beside the title. */
export function SettingsGroup(props: { title: string; action?: JSX.Element; children: JSX.Element }) {
    return (
        <section>
            <div class="settings-group-head mb-1.5 flex items-center justify-between gap-3">
                <div class="text-[length:calc(0.68rem*var(--settings-scale,1))] font-semibold tracking-wide text-ink-faint uppercase">
                    {props.title}
                </div>
                {props.action}
            </div>
            <div class="border-y border-edge/80">{props.children}</div>
        </section>
    );
}

/** A label/description pair on the left with its control on the right; a control too wide to fit wraps under the text. */
export function SettingsRow(props: {
    title: string;
    description: string;
    children: JSX.Element;
    disabled?: boolean;
    onClick?: () => void;
}) {
    return (
        <div
            class="settings-row flex min-h-13 flex-wrap items-center gap-x-4 gap-y-2 border-b border-edge/70 px-1 py-2.5 last:border-b-0 sm:flex-nowrap"
            classList={{
                "cursor-pointer hover:bg-raised/40": !!props.onClick && !props.disabled,
                "opacity-50": !!props.disabled,
            }}
            onClick={() => !props.disabled && props.onClick?.()}
        >
            <div class="settings-row-copy min-w-0 flex-[1_1_12rem]">
                <div class="text-[length:calc(0.82rem*var(--settings-scale,1))] font-medium text-ink">
                    {props.title}
                </div>
                <div class="mt-0.5 text-[length:calc(0.72rem*var(--settings-scale,1))] leading-relaxed text-ink-faint">
                    {props.description}
                </div>
            </div>
            <div class="settings-row-control ml-auto shrink-0">{props.children}</div>
        </div>
    );
}

/** The one width every dropdown in Settings takes, so a column of them lines up. */
export const pickerWidth = "13rem";

export const settingsIconButton =
    "flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink";
/** One entry in a disclosure's panel: what it is on the left, its value and a remove button on the right. */
export const panelLine = "group flex items-center gap-3 border-b border-edge/60 px-3 py-1.5";
/** A remove button that shows while its entry is hovered or focused. */
export const panelRemove = `${settingsIconButton} opacity-0 group-hover:opacity-100 focus-visible:opacity-100`;
/** The form that adds an entry, last in a disclosure's panel. */
export const panelForm = "flex flex-wrap items-center gap-2 p-2";

/**
 * A row that opens a panel of entries under it, as Web search lists a provider's keys: the row or its chevron
 * opens it. `fixed` is a row with nothing to open; `actions` sit before the chevron.
 */
export function SettingsDisclosure(props: {
    title: string;
    description: string;
    label: string;
    actions?: JSX.Element;
    fixed?: boolean;
    children: JSX.Element;
}) {
    const [open, setOpen] = createSignal(false);

    return (
        <div class="border-b border-edge/70 last:border-b-0 [&>.settings-row]:border-b-0">
            <SettingsRow
                title={props.title}
                description={props.description}
                onClick={props.fixed ? undefined : () => setOpen(!open())}
            >
                <div class="flex items-center gap-1">
                    {props.actions}
                    <Show when={!props.fixed}>
                        <button
                            class={settingsIconButton}
                            aria-expanded={open()}
                            aria-label={props.label}
                            onClick={(event) => {
                                event.stopPropagation();
                                setOpen(!open());
                            }}
                        >
                            <Chevron open={open()} />
                        </button>
                    </Show>
                </div>
            </SettingsRow>
            <Show when={open() && !props.fixed}>
                <div class="mx-1 mb-3 overflow-hidden rounded-lg border border-edge bg-surface/55">
                    {props.children}
                </div>
            </Show>
        </div>
    );
}
