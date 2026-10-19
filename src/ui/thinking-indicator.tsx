import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack, type JSX } from "solid-js";
import { pluginThinkingPrefix, thinkingPrefs, type ThinkingPrefs } from "../state/prefs";
import { TextShimmer } from "./text-shimmer";
import { t } from "../state/i18n";
import {
    thinkingIndicator,
    type ThinkingContext,
    type ThinkingDrawing,
    type ThinkingIndicator as ThinkingIndicatorPlugin,
} from "../thinking-indicators";

const speedScale = { slow: 1.6, normal: 1, fast: 0.6 } as const;
/** How many pieces each built-in figure is drawn from; Shimmer and Still have none. */
const figurePieces: Record<string, number> = { orbs: 3, float: 5, dots: 3, pulse: 1 };

/** The style to draw: a plugin's only while it is registered, Shimmer in its place otherwise. */
export function shownThinkingStyle(style: string, registered: (id: string) => boolean) {
    if (!style.startsWith(pluginThinkingPrefix)) return style;
    return registered(style.slice(pluginThinkingPrefix.length)) ? style : "shimmer";
}

/** Whether the word shows beside the figure: Shimmer and Still are the word. */
export function thinkingWordShown(style: string, label: boolean) {
    return style === "shimmer" || style === "still" || (!!figurePieces[style] && label);
}

export function ThinkingIndicator(props: {
    heading?: string;
    sessionId?: string | null;
    prefs?: ThinkingPrefs;
    preview?: boolean;
}) {
    const prefs = () => props.prefs ?? thinkingPrefs();
    const style = () => shownThinkingStyle(prefs().style, (id) => !!thinkingIndicator(id));
    const label = () => t("drift.chat.thinking");
    const plugin = () => (style().startsWith(pluginThinkingPrefix) ? style().slice(pluginThinkingPrefix.length) : "");
    const registered = () => (plugin() ? thinkingIndicator(plugin()) : undefined);
    const context = createMemo<ThinkingContext>(() => ({
        sessionId: props.sessionId ?? null,
        label: label(),
        tint: prefs().tint,
        speed: speedScale[prefs().speed],
        preview: !!props.preview,
    }));
    const builtIn = () => <BuiltIn look={style()} label={label()} word={thinkingWordShown(style(), prefs().label)} />;

    return (
        <span
            class="thinking-indicator"
            data-style={style()}
            data-tint={prefs().tint}
            style={{ "--thinking-speed": String(speedScale[prefs().speed]) }}
        >
            <Show when={registered()} keyed fallback={builtIn()}>
                {(indicator) => (
                    <PluginIndicator
                        id={plugin()}
                        indicator={indicator}
                        context={context()}
                        fallback={<BuiltIn look="shimmer" label={label()} word />}
                    />
                )}
            </Show>
            <Show when={props.heading}>{(heading) => <span class="timeline-thinking-heading">{heading()}</span>}</Show>
        </span>
    );
}

function BuiltIn(props: { look: string; label: string; word: boolean }) {
    const pieces = () => Array.from({ length: figurePieces[props.look] ?? 0 }, (_, index) => index);
    return (
        <>
            <Show when={pieces().length}>
                <span class="thinking-figure" aria-hidden="true">
                    <For each={pieces()}>
                        {(index) => <span class="thinking-piece" style={{ "--i": String(index) }} />}
                    </For>
                </span>
            </Show>
            <Show when={props.word}>
                <Show when={props.look === "shimmer"} fallback={<span class="thinking-label">{props.label}</span>}>
                    <TextShimmer text={props.label} />
                </Show>
            </Show>
        </>
    );
}

/** A plugin's indicator in its own element, drawn afresh for each registration; one that throws gives way to Shimmer, and is logged. */
function PluginIndicator(props: {
    id: string;
    indicator: ThinkingIndicatorPlugin;
    context: ThinkingContext;
    fallback: JSX.Element;
}) {
    let host!: HTMLSpanElement;
    let drawing: ThinkingDrawing;
    const [failed, setFailed] = createSignal(false);
    const guarded = (step: string, work: () => void) => {
        try {
            work();
        } catch (error) {
            console.warn(`[Drift] Thinking indicator ${props.id} failed to ${step}`, error);
            setFailed(true);
        }
    };

    onMount(() => guarded("render", () => (drawing = untrack(() => props.indicator.render(host, props.context)))));
    createEffect(
        on(
            () => props.context,
            (context) => {
                const current = drawing;
                if (current && typeof current === "object" && current.update)
                    guarded("update", () => current.update?.(context));
            },
            { defer: true },
        ),
    );
    onCleanup(() => guarded("dispose", () => (typeof drawing === "function" ? drawing() : drawing?.dispose?.())));

    return (
        <Show when={!failed()} fallback={props.fallback}>
            <span ref={host} class="thinking-plugin" />
        </Show>
    );
}
