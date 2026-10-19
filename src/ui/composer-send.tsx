import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { IconArrowUp, IconListPlus } from "./icons";
import { t } from "../state/i18n";

/** How long a touch holds the send button before it queues instead. */
const longPressMs = 500;

/**
 * The composer's send button. While a turn runs it steers; holding Alt turns it into queue, and on a touch screen a
 * long press queues.
 */
export function ComposerSend(props: { busy: boolean; disabled: boolean; onSend: () => void; onQueue: () => void }) {
    const [altHeld, setAltHeld] = createSignal(false);
    const queues = () => props.busy && altHeld();
    let pressTimer: ReturnType<typeof setTimeout> | undefined;
    // Set once a long press has queued, so the click that ends it does not send as well.
    let pressQueued = false;

    // Alt is read from every key event, so a release outside the window still clears it.
    onMount(() => {
        const track = (event: KeyboardEvent) => setAltHeld(event.altKey);
        const release = () => setAltHeld(false);
        window.addEventListener("keydown", track);
        window.addEventListener("keyup", track);
        window.addEventListener("blur", release);
        onCleanup(() => {
            window.removeEventListener("keydown", track);
            window.removeEventListener("keyup", track);
            window.removeEventListener("blur", release);
        });
    });
    onCleanup(() => clearTimeout(pressTimer));

    const startPress = (event: PointerEvent) => {
        pressQueued = false;
        if (event.pointerType !== "touch" || !props.busy || props.disabled) return;

        pressTimer = setTimeout(() => {
            pressTimer = undefined;
            pressQueued = true;
            props.onQueue();
        }, longPressMs);
    };
    const endPress = () => {
        clearTimeout(pressTimer);
        pressTimer = undefined;
    };
    const click = () => {
        if (pressQueued) {
            pressQueued = false;
            return;
        }

        if (queues()) props.onQueue();
        else props.onSend();
    };

    const label = () => {
        if (queues()) return t("drift.prompt.queue");

        return props.busy ? t("drift.prompt.steer") : t("prompt.action.send");
    };

    return (
        <button
            class="composer-submit flex size-7 items-center justify-center rounded-md text-accent transition-colors hover:bg-raised disabled:pointer-events-none disabled:text-ink-faint disabled:opacity-60"
            title={props.busy && !queues() ? t("drift.prompt.steerHint") : label()}
            aria-label={label()}
            disabled={props.disabled}
            onClick={click}
            onPointerDown={startPress}
            onPointerUp={endPress}
            onPointerLeave={endPress}
            onPointerCancel={endPress}
            // A long press would otherwise open the phone's text or link menu.
            onContextMenu={(event) => event.preventDefault()}
        >
            <Show when={queues()} fallback={<IconArrowUp class="size-4" />}>
                <IconListPlus class="size-4" />
            </Show>
        </button>
    );
}
