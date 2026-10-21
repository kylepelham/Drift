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
/** How many pieces each built-in figure is drawn from. */
const figurePieces: Record<string, number> = { orbs: 3, float: 5, dots: 3, pulse: 1 };

/** The style to draw: a built-in or a registered plugin's, Orbs in place of anything else. */
export function shownThinkingStyle(style: string, registered: (id: string) => boolean) {
    if (style.startsWith(pluginThinkingPrefix))
        return registered(style.slice(pluginThinkingPrefix.length)) ? style : "orbs";
    return figurePieces[style] ? style : "orbs";
}

/** The text after the indicator, the translated word Thinking unless the user wrote their own. */
export function thinkingText(prefs: Pick<ThinkingPrefs, "text">) {
    return prefs.text.trim() || t("drift.chat.thinking");
}

export function ThinkingIndicator(props: {
    heading?: string;
    sessionId?: string | null;
    prefs?: ThinkingPrefs;
    preview?: boolean;
}) {
    const prefs = () => props.prefs ?? thinkingPrefs();
    const style = () => shownThinkingStyle(prefs().style, (id) => !!thinkingIndicator(id));
    const label = () => thinkingText(prefs());
    const plugin = () => (style().startsWith(pluginThinkingPrefix) ? style().slice(pluginThinkingPrefix.length) : "");
    const registered = () => (plugin() ? thinkingIndicator(plugin()) : undefined);
    const context = createMemo<ThinkingContext>(() => ({
        sessionId: props.sessionId ?? null,
        label: label(),
        tint: prefs().tint,
        speed: speedScale[prefs().speed],
        preview: !!props.preview,
    }));

    return (
        <span
            class="thinking-indicator"
            data-tint={prefs().tint}
            style={{ "--thinking-speed": String(speedScale[prefs().speed]) }}
        >
            <Show when={registered()} keyed fallback={<Figure look={style()} />}>
                {(indicator) => (
                    <PluginIndicator
                        id={plugin()}
                        indicator={indicator}
                        context={context()}
                        fallback={<Figure look="orbs" />}
                    />
                )}
            </Show>
            <Show when={prefs().label} fallback={<span class="sr-only">{label()}</span>}>
                <Show when={prefs().shimmer} fallback={<span class="thinking-label">{label()}</span>}>
                    <TextShimmer text={label()} />
                </Show>
            </Show>
            <Show when={props.heading}>{(heading) => <span class="timeline-thinking-heading">{heading()}</span>}</Show>
        </span>
    );
}

function Figure(props: { look: string }) {
    const pieces = () => Array.from({ length: figurePieces[props.look] ?? 0 }, (_, index) => index);
    return (
        <span class="thinking-figure" data-look={props.look} aria-hidden="true">
            <For each={pieces()}>{(index) => <span class="thinking-piece" style={{ "--i": String(index) }} />}</For>
        </span>
    );
}

/** A plugin's indicator in its own element, drawn afresh for each registration; one that throws gives way to Orbs, and is logged. */
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
