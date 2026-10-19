/**
 * Small presentational controls shared across unrelated feature modules.
 *
 * These previously lived inside whichever feature happened to need them first (Chevron in the tool
 * output renderer, Toggle in the model manager), which made the import graph imply relationships
 * that do not exist.
 */

import { createSignal, For } from "solid-js";

import type { JSX } from "solid-js";

/** Disclosure arrow that rotates to point down when its section is open. */
export function Chevron(props: { open: boolean }) {
    return (
        <svg
            class="size-3 shrink-0 transition-transform duration-150"
            classList={{ "rotate-90": props.open }}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
        >
            <path d="M6 4l4 4-4 4" />
        </svg>
    );
}

/**
 * Switch-style toggle. Pointer and click events are stopped so the toggle stays usable inside
 * draggable or clickable rows without triggering the row's own handler.
 */
export function Toggle(props: {
    label: string;
    title?: string;
    checked: boolean;
    disabled?: boolean;
    onChange: () => void;
}) {
    return (
        <button
            role="switch"
            aria-label={props.label}
            title={props.title}
            aria-checked={props.checked}
            disabled={props.disabled}
            class="shrink-0 disabled:opacity-50"
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
                event.stopPropagation();
                props.onChange();
            }}
        >
            <ToggleTrack checked={props.checked} />
        </button>
    );
}

/**
 * The visual track and knob on their own, for rows that already act as the switch and only need
 * the indicator rather than a nested button.
 */
export function ToggleTrack(props: { checked: boolean }) {
    return (
        <span
            class="relative block h-4 w-7 shrink-0 rounded-full border transition-colors"
            classList={{
                "border-accent bg-accent": props.checked,
                "border-edge-strong bg-raised": !props.checked,
            }}
        >
            <span
                class="absolute top-0.5 left-0.5 size-2.5 rounded-full transition-[transform,background-color]"
                classList={{
                    "translate-x-3 bg-accent-ink": props.checked,
                    "bg-ink-muted": !props.checked,
                }}
            />
        </span>
    );
}

/** A small label: neutral, accent or warning. */
export function Badge(props: { tone?: "accent" | "warn"; children: JSX.Element }) {
    return (
        <span
            class="rounded px-1.5 py-0.5 text-[length:calc(0.65rem*var(--ui-text-scale,1))]"
            classList={{
                "bg-raised text-ink-muted": !props.tone,
                "bg-accent/12 text-accent": props.tone === "accent",
                "bg-warn/12 text-warn": props.tone === "warn",
            }}
        >
            {props.children}
        </span>
    );
}

/** A choice among a few options shown side by side, as one radio group. */
export function Segmented<T extends string>(props: {
    label: string;
    options: readonly { id: T; label: string }[];
    value: T;
    onChange: (id: T) => void;
}) {
    return (
        <div
            role="radiogroup"
            aria-label={props.label}
            class="inline-flex rounded-lg border border-edge bg-bg/60 p-0.5"
        >
            <For each={props.options}>
                {(option) => (
                    <button
                        role="radio"
                        aria-checked={option.id === props.value}
                        class="rounded-md px-3 py-1 text-xs font-medium transition-colors"
                        classList={{
                            "bg-raised text-ink shadow-sm shadow-black/20": option.id === props.value,
                            "text-ink-muted hover:text-ink": option.id !== props.value,
                        }}
                        onClick={() => props.onChange(option.id)}
                    >
                        {option.label}
                    </button>
                )}
            </For>
        </div>
    );
}

/**
 * A range from `min` to `max`, its filled part in the accent. With `onDragging`, a pointer drag reports each value
 * there and `onInput` only the one it is let go at: for a value that resizes the slider itself, such as the zoom.
 */
export function Slider(props: {
    label: string;
    min: number;
    max: number;
    step: number;
    value: number;
    onInput: (value: number) => void;
    onDragging?: (value: number | undefined) => void;
}) {
    const [dragging, setDragging] = createSignal(false);
    const fill = () => `${((props.value - props.min) / (props.max - props.min)) * 100}%`;
    const settle = (value: number) => {
        setDragging(false);
        props.onDragging?.(undefined);
        props.onInput(value);
    };

    return (
        <input
            type="range"
            class="drift-slider"
            aria-label={props.label}
            min={props.min}
            max={props.max}
            step={props.step}
            value={props.value}
            style={{ "--fill": fill() }}
            onPointerDown={() => props.onDragging && setDragging(true)}
            onInput={(event) => {
                const value = Number(event.currentTarget.value);
                if (dragging()) props.onDragging?.(value);
                else props.onInput(value);
            }}
            onPointerUp={(event) => dragging() && settle(Number(event.currentTarget.value))}
            onPointerCancel={(event) => dragging() && settle(Number(event.currentTarget.value))}
        />
    );
}
