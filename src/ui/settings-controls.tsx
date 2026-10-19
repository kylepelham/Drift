import type { JSX } from "solid-js";

/**
 * Layout primitives shared by the settings sections. They live here rather than in settings.tsx so a
 * section can be split into its own file without importing back into the module that renders it.
 */

/** A titled block of rows, with an optional control beside the title. */
export function SettingsGroup(props: { title: string; action?: JSX.Element; children: JSX.Element }) {
    return (
        <section>
            <div class="mb-1.5 flex items-center justify-between gap-3">
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
