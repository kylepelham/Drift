import { createSignal } from "solid-js";

/** What a plugin's indicator is drawn with, and given again whenever it changes; Drift shows the heading beside it. */
export type ThinkingContext = {
    sessionId: string | null;
    /** The translated word for thinking, for indicators that show one. */
    label: string;
    tint: "neutral" | "accent" | "spectrum";
    /** Multiplies animation durations: above 1 is slower. */
    speed: number;
    /** Drawn in Settings rather than in a transcript. */
    preview: boolean;
};

export type ThinkingDrawing =
    void | (() => void) | { update?: (context: ThinkingContext) => void; dispose?: () => void };

export type ThinkingIndicator = {
    /** Shown in the Settings picker. */
    name: string;
    render: (host: HTMLElement, context: ThinkingContext) => ThinkingDrawing;
};

/** One registration; its own object, so each unregister removes only what it added. */
type Registration = { indicator: ThinkingIndicator };

const indicators = new Map<string, Registration>();
// `indicators` is a plain Map; reading the version subscribes to register and unregister.
const [version, setVersion] = createSignal(0);

export function registerThinkingIndicator(id: string, indicator: ThinkingIndicator) {
    if (!id.trim() || typeof indicator?.render !== "function") throw new Error("an indicator needs an id and a render");
    const registration: Registration = { indicator };
    indicators.set(id, registration);
    setVersion((value) => value + 1);

    return () => {
        if (indicators.get(id) !== registration) return;
        indicators.delete(id);
        setVersion((value) => value + 1);
    };
}

export function thinkingIndicator(id: string) {
    version();
    return indicators.get(id)?.indicator;
}

export function thinkingIndicators() {
    version();
    return [...indicators].map(([id, { indicator }]) => ({ id, name: indicator.name || id }));
}
