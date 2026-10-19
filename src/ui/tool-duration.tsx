import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import { t } from "../state/i18n";

export type ToolTimingState = {
    status: string;
    time?: { start?: number; end?: number };
};

const clockSubscribers = new Set<(now: number) => void>();
let clockTimer: ReturnType<typeof setInterval> | undefined;

function timestamp(value: unknown) {
    return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function running(state: ToolTimingState) {
    return state.status === "pending" || state.status === "running";
}

function subscribeClock(subscriber: (now: number) => void) {
    clockSubscribers.add(subscriber);
    subscriber(Date.now());
    clockTimer ??= setInterval(() => {
        const now = Date.now();
        for (const update of clockSubscribers) update(now);
    }, 1000);
    return () => {
        clockSubscribers.delete(subscriber);
        if (clockSubscribers.size || !clockTimer) return;
        clearInterval(clockTimer);
        clockTimer = undefined;
    };
}

function createSharedNow(active: () => boolean) {
    const [now, setNow] = createSignal(Date.now());
    createEffect(() => {
        if (!active()) return;
        const unsubscribe = subscribeClock(setNow);
        onCleanup(unsubscribe);
    });
    return now;
}

export function toolElapsedMs(state: ToolTimingState, now: number, maxMs?: number) {
    const start = timestamp(state.time?.start);
    if (start === undefined) return undefined;
    const end = timestamp(state.time?.end);
    if (end === undefined && !running(state)) return undefined;
    const elapsed = Math.max(0, (end ?? now) - start);
    return maxMs === undefined ? elapsed : Math.min(elapsed, Math.max(0, maxMs));
}

export function formatToolDuration(ms: number) {
    const totalSeconds = Math.max(0, Math.floor(ms / 1000));
    if (totalSeconds < 60) return t("drift.message.duration.seconds", { seconds: totalSeconds });
    const totalMinutes = Math.floor(totalSeconds / 60);
    const pad = (value: number) => String(value).padStart(2, "0");
    if (totalMinutes < 60)
        return t("drift.message.duration.minutes", { minutes: totalMinutes, seconds: pad(totalSeconds % 60) });
    return t("drift.message.duration.hours", { hours: Math.floor(totalMinutes / 60), minutes: pad(totalMinutes % 60) });
}

/** The label a row shows; under a second says nothing worth the space. */
export function visibleToolDuration(ms: number | undefined) {
    return ms === undefined || ms < 1000 ? undefined : formatToolDuration(ms);
}

export function ToolDuration(props: { state: ToolTimingState; maxMs?: number }) {
    const live = () =>
        props.maxMs === undefined &&
        running(props.state) &&
        timestamp(props.state.time?.start) !== undefined &&
        timestamp(props.state.time?.end) === undefined;
    const now = createSharedNow(live);
    const elapsed = () => toolElapsedMs(props.state, now(), props.maxMs);
    const duration = () => visibleToolDuration(elapsed());
    return (
        <Show when={duration()} keyed>
            {(value) => <span class="shrink-0 font-mono text-xs text-ink-faint">{value}</span>}
        </Show>
    );
}

/** Wall time at least one call ran: calls side by side count once, the gaps between them not at all. */
export function coveredMs(states: ToolTimingState[], now: number) {
    const spans = states
        .flatMap((state) => {
            const start = timestamp(state.time?.start);
            const end = timestamp(state.time?.end) ?? (running(state) ? now : undefined);
            return start === undefined || end === undefined ? [] : [[start, Math.max(start, end)] as const];
        })
        .sort((a, b) => a[0] - b[0]);
    if (!spans.length) return undefined;

    let total = 0;
    let [from, to] = spans[0];
    for (const [start, end] of spans.slice(1)) {
        if (start <= to) {
            to = Math.max(to, end);
            continue;
        }
        total += to - from;
        [from, to] = [start, end];
    }

    return total + to - from;
}

/** A group's time, ticking while any of its calls still runs. */
export function CoveredDuration(props: { states: ToolTimingState[] }) {
    const live = () => props.states.some((state) => running(state) && timestamp(state.time?.start) !== undefined);
    const now = createSharedNow(live);
    const duration = () => visibleToolDuration(coveredMs(props.states, now()));
    return (
        <Show when={duration()} keyed>
            {(value) => <span class="shrink-0 font-mono text-xs text-ink-faint">{value}</span>}
        </Show>
    );
}
