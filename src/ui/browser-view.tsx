import { buttonName, composesOrPastes, modifierBits, pagePoint, paneFiles } from "./browser-input";
import { createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { reservedKey } from "../state/keybinds";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { BrowserFrame, BrowserView as ViewSocket } from "../engine/browser-view";

type PageSize = { width: number; height: number };

/** How long the pane waits for a resize to settle before the page is laid out again. */
const FIT_DELAY_MS = 150;

/** The browser's active page as the engine streams it; clicks, scrolling and typing here go to the page. */
export function BrowserView(props: {
    sessionId: string;
    /** The tab the user or agent made active; until a picture of it arrives the pane shows it is on its way. */
    activePage?: string;
    onReady: (view: ViewSocket) => void;
}) {
    const engine = useEngine();
    const [size, setSize] = createSignal<PageSize>();
    const [shownPage, setShownPage] = createSignal<string>();
    const [cursor, setCursor] = createSignal("default");
    const switching = () => !!size() && !!props.activePage && shownPage() !== props.activePage;
    let area!: HTMLDivElement;
    let canvas!: HTMLCanvasElement;
    // Keys go to a hidden text box: a desktop webview fires paste only into an editable element, and input methods compose there.
    let keys!: HTMLTextAreaElement;
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
            setShownPage(frame.page);
        }
        drawing = false;
    }

    // The page is laid out to fill the area the pane gives it.
    function fit() {
        const width = area.clientWidth;
        const height = area.clientHeight;
        if (width > 0 && height > 0) view.send({ type: "fit", width, height });
    }

    // The pane is rebuilt for another session, so the socket is opened once for this one.
    const view = untrack(() =>
        engine.actions.watchBrowser(props.sessionId, {
            onFrame: (frame) => {
                latest = frame;
                if (!drawing) void draw();
            },
            onCursor: setCursor,
            // The browser's clipboard is its own, so what the user copied there is put on theirs.
            onCopied: (text) => void navigator.clipboard.writeText(text),
            onOpen: fit,
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
        const { buttons } = event;
        view.send({ type: "mouse", event: kind, ...at, button, buttons, clickCount, modifiers: modifierBits(event) });
    }

    // Moves are sent at most once a frame.
    function move(event: MouseEvent) {
        const scheduled = pendingMove !== undefined;
        pendingMove = event;
        if (scheduled) return;

        requestAnimationFrame(flushMove);
    }

    function flushMove() {
        if (pendingMove) mouse("move", pendingMove);
        pendingMove = undefined;
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
        if (composesOrPastes(event) || reservedKey(event)) return;

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
        const data = event.clipboardData;
        event.preventDefault();
        if (!data) return;
        const text = data.getData("text/plain");
        void sendFiles([...data.files], (files) => {
            if (text || files.length) view.send({ type: "paste", text, files });
        });
    }

    // What an input method or dead key settled on is typed into the page; the box is kept empty.
    function composed(text: string) {
        keys.value = "";
        if (text) view.send({ type: "text", text });
    }

    function droppable(event: DragEvent) {
        const types = event.dataTransfer?.types ?? [];
        if (!types.includes("Files") && !types.includes("text/plain")) return;
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    }

    function drop(event: DragEvent) {
        const data = event.dataTransfer;
        const at = point(event);
        if (!data || !at) return;
        event.preventDefault();
        const text = data.files.length ? "" : data.getData("text/plain");
        void sendFiles([...data.files], (files) => view.send({ type: "drop", ...at, text, files }));
        keys.focus({ preventScroll: true });
    }

    async function sendFiles(list: File[], send: (files: Awaited<ReturnType<typeof paneFiles>>["files"]) => void) {
        const { files, skipped } = await paneFiles(list);
        send(files);
        if (skipped.length)
            engine.actions.notice({
                message: t("drift.browser.filesTooLarge", { files: skipped.join(", ") }),
                variant: "warning",
            });
    }

    onMount(() => {
        canvas.addEventListener("wheel", wheel, { passive: false });
        onCleanup(() => canvas.removeEventListener("wheel", wheel));

        let settling: ReturnType<typeof setTimeout> | undefined;
        const resized = new ResizeObserver(() => {
            clearTimeout(settling);
            settling = setTimeout(fit, FIT_DELAY_MS);
        });
        resized.observe(area);
        onCleanup(() => {
            clearTimeout(settling);
            resized.disconnect();
        });
    });

    return (
        <div
            ref={area}
            class="browser-view relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-raised/40"
        >
            <textarea
                ref={keys}
                data-own-keys
                class="pointer-events-none absolute top-0 left-0 size-px resize-none opacity-0"
                aria-label={t("drift.browser.view")}
                autocomplete="off"
                spellcheck={false}
                onKeyDown={(event) => key("down", event)}
                onKeyUp={(event) => key("up", event)}
                onPaste={paste}
                onCompositionEnd={(event) => composed(event.data)}
                onInput={(event) => {
                    if (!event.isComposing) composed(keys.value);
                }}
            />
            <canvas
                ref={canvas}
                class="block max-h-full max-w-full outline-none"
                classList={{ invisible: !size() }}
                style={{ cursor: cursor() }}
                onMouseDown={(event) => {
                    // The press is the page's: Drift neither selects nor moves focus off the box.
                    event.preventDefault();
                    keys.focus({ preventScroll: true });
                    mouse("down", event, event.detail || 1);
                }}
                onMouseUp={(event) => {
                    // A move still waiting for its frame goes first, so the page never sees it after the release.
                    flushMove();
                    mouse("up", event, event.detail || 1);
                }}
                onMouseMove={move}
                onContextMenu={(event) => event.preventDefault()}
                onDragOver={droppable}
                onDrop={drop}
            />
            <Show when={!size()}>
                <p class="absolute inset-x-0 top-1/3 text-center text-sm text-ink-faint">
                    {t("drift.browser.waiting")}
                </p>
            </Show>
            <Show when={switching()}>
                <div
                    class="browser-loading pointer-events-none absolute inset-0 bg-bg/40"
                    role="status"
                    aria-label={t("drift.browser.loading")}
                >
                    <div class="absolute inset-x-0 top-0 h-0.5 overflow-hidden bg-accent/20">
                        <div class="browser-loading-bar h-full w-2/5 bg-accent" />
                    </div>
                </div>
            </Show>
        </div>
    );
}
