import { buttonName, isPasteShortcut, modifierBits, pagePoint } from "./browser-input";
import { createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { BrowserFrame, BrowserView as ViewSocket } from "../engine/browser-view";

type PageSize = { width: number; height: number };

/** The browser's active page as the engine streams it; clicks, scrolling and typing here go to the page. */
export function BrowserView(props: { sessionId: string; onReady: (view: ViewSocket) => void }) {
    const engine = useEngine();
    const [size, setSize] = createSignal<PageSize>();
    let canvas!: HTMLCanvasElement;
    let latest: BrowserFrame | undefined;
    let drawing = false;
    let pendingMove: MouseEvent | undefined;

    // Only the newest frame is drawn; any that arrive while one decodes are skipped.
    async function draw() {
        drawing = true;
        while (latest) {
            const frame = latest;
            latest = undefined;
            const bitmap = await createImageBitmap(frame.jpeg).catch(() => undefined);
            if (!bitmap) continue;

            canvas.width = bitmap.width;
            canvas.height = bitmap.height;
            canvas.getContext("2d")?.drawImage(bitmap, 0, 0);
            bitmap.close();
            setSize({ width: frame.width, height: frame.height });
        }
        drawing = false;
    }

    // The pane is rebuilt for another session, so the socket is opened once for this one.
    const view = untrack(() =>
        engine.actions.watchBrowser(props.sessionId, (frame) => {
            latest = frame;
            if (!drawing) void draw();
        }),
    );
    untrack(() => props.onReady(view));
    onCleanup(() => view.close());

    function point(event: MouseEvent) {
        const page = size();
        if (!page) return undefined;

        return pagePoint(event, canvas.getBoundingClientRect(), page);
    }

    function mouse(kind: "down" | "up" | "move", event: MouseEvent, clickCount?: number) {
        const at = point(event);
        if (!at) return;

        const button = buttonName(event.button);
        view.send({ type: "mouse", event: kind, ...at, button, clickCount, modifiers: modifierBits(event) });
    }

    // Moves are sent at most once a frame.
    function move(event: MouseEvent) {
        const scheduled = pendingMove !== undefined;
        pendingMove = event;
        if (scheduled) return;

        requestAnimationFrame(() => {
            if (pendingMove) mouse("move", pendingMove);
            pendingMove = undefined;
        });
    }

    function wheel(event: WheelEvent) {
        const at = point(event);
        if (!at) return;

        event.preventDefault();
        view.send({
            type: "mouse",
            event: "wheel",
            ...at,
            deltaX: event.deltaX,
            deltaY: event.deltaY,
            modifiers: modifierBits(event),
        });
    }

    function key(kind: "down" | "up", event: KeyboardEvent) {
        if (event.isComposing || isPasteShortcut(event)) return;

        event.preventDefault();
        view.send({
            type: "key",
            event: kind,
            key: event.key,
            code: event.code,
            keyCode: event.keyCode,
            modifiers: modifierBits(event),
        });
    }

    function paste(event: ClipboardEvent) {
        const text = event.clipboardData?.getData("text") ?? "";
        event.preventDefault();
        if (text) view.send({ type: "text", text });
    }

    onMount(() => {
        canvas.addEventListener("wheel", wheel, { passive: false });
        onCleanup(() => canvas.removeEventListener("wheel", wheel));
    });

    return (
        <div class="browser-view relative flex min-h-0 flex-1 items-start justify-center overflow-hidden bg-raised/40">
            <canvas
                ref={canvas}
                tabIndex={0}
                class="block max-h-full max-w-full cursor-default outline-none"
                classList={{ invisible: !size() }}
                aria-label={t("drift.browser.view")}
                onMouseDown={(event) => {
                    canvas.focus();
                    mouse("down", event, event.detail || 1);
                }}
                onMouseUp={(event) => mouse("up", event, event.detail || 1)}
                onMouseMove={move}
                onContextMenu={(event) => event.preventDefault()}
                onKeyDown={(event) => key("down", event)}
                onKeyUp={(event) => key("up", event)}
                onPaste={paste}
            />
            <Show when={!size()}>
                <p class="absolute inset-x-0 top-1/3 text-center text-sm text-ink-faint">
                    {t("drift.browser.waiting")}
                </p>
            </Show>
        </div>
    );
}
