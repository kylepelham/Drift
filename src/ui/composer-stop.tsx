import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { IconStop } from "./icons";
import { t } from "../state/i18n";

/** How long the button that stops background work waits before it takes a press, so a double press stops only the turn. */
const armMs = 500;

/**
 * Stop. While the turn runs it stops the turn alone, and background workers and commands run on and still wake the
 * conversation; once the turn is idle and some still run, it names them and stops them all.
 */
export function ComposerStop(props: {
    busy: boolean;
    workers: number;
    commands: number;
    onStopTurn: () => void;
    onStopAll: () => void;
}) {
    const working = () => props.workers + props.commands > 0;
    const stopsWork = () => !props.busy && working();
    const [armed, setArmed] = createSignal(true);
    let timer: ReturnType<typeof setTimeout> | undefined;

    createEffect(
        on(stopsWork, (now, before) => {
            clearTimeout(timer);
            if (!now || before === undefined) return setArmed(true);
            setArmed(false);
            timer = setTimeout(() => setArmed(true), armMs);
        }),
    );
    onCleanup(() => clearTimeout(timer));

    const label = () => {
        const workers = t(props.workers === 1 ? "drift.stop.workers.one" : "drift.stop.workers.other", {
            count: props.workers,
        });
        const commands = t(props.commands === 1 ? "drift.stop.commands.one" : "drift.stop.commands.other", {
            count: props.commands,
        });
        if (!props.commands) return t("drift.stop.work", { work: workers });
        if (!props.workers) return t("drift.stop.work", { work: commands });
        return t("drift.stop.work", { work: t("drift.stop.both", { workers, commands }) });
    };

    return (
        <Show when={props.busy || working()}>
            <Show
                when={stopsWork()}
                fallback={
                    <button
                        class="composer-stop flex size-7 items-center justify-center rounded-md text-ink-muted transition-colors hover:bg-raised hover:text-danger"
                        title={t("prompt.action.stop")}
                        aria-label={t("prompt.action.stop")}
                        onClick={() => props.onStopTurn()}
                    >
                        <IconStop class="size-3.5" />
                    </button>
                }
            >
                <button
                    class="composer-stop flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-ink-muted transition-colors hover:bg-raised hover:text-danger disabled:cursor-default disabled:opacity-60"
                    title={label()}
                    disabled={!armed()}
                    onClick={() => props.onStopAll()}
                >
                    <IconStop class="size-3.5" />
                    {label()}
                </button>
            </Show>
        </Show>
    );
}
