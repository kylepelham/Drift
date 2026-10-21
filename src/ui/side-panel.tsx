import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { nativeFrame, WindowControls } from "./titlebar";
import { Show, untrack, type JSX } from "solid-js";
import { IconChevronLeft, IconX } from "./icons";
import { persisted } from "../state/persist";
import { t } from "../state/i18n";

const minWidth = 360;

/** A panel right of the chat: resizable by its left edge, its header the title bar with the window controls; full screen on a phone. */
export function SidePanel(props: {
    /** Keys its saved width. */
    name: string;
    defaultWidth: number;
    title: JSX.Element;
    actions?: JSX.Element;
    /** The window's own controls; only the rightmost panel shows them. Default true. */
    windowControls?: boolean;
    onClose: () => void;
    children: JSX.Element;
}) {
    const narrow = createMediaQuery(smallScreenQuery);
    // A panel's name and default never change while it is open, so its saved width is read once.
    const [width, setWidth] = untrack(() => persisted<number>(`drift.${props.name}.width`, props.defaultWidth));
    const drag = nativeFrame() ? "" : undefined;

    return (
        <div
            class="side-panel relative flex min-h-0 min-w-0 shrink-0 flex-col overflow-hidden border-l border-edge bg-bg"
            style={narrow() ? undefined : { width: `${width()}px` }}
        >
            <Show when={!narrow()}>
                <ResizeHandle width={width()} onWidth={setWidth} />
            </Show>
            <div
                class="side-panel-header flex h-11 shrink-0 items-center gap-1 border-b border-edge/60 pl-2"
                classList={{ "pr-2": !nativeFrame() || narrow() }}
                data-tauri-drag-region={drag}
            >
                <Show when={narrow()}>
                    <button
                        class="flex size-11 shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-raised hover:text-ink"
                        title={t("common.goBack")}
                        onClick={() => props.onClose()}
                    >
                        <IconChevronLeft />
                    </button>
                </Show>
                {props.title}
                <div class="min-w-2 flex-1 self-stretch" data-tauri-drag-region={drag} />
                {props.actions}
                <Show when={!narrow()}>
                    <button
                        class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        title={t("common.close")}
                        onClick={() => props.onClose()}
                    >
                        <IconX class="size-3.5" />
                    </button>
                    <Show when={props.windowControls ?? true}>
                        <WindowControls />
                    </Show>
                </Show>
            </div>
            {props.children}
        </div>
    );
}

function ResizeHandle(props: { width: number; onWidth: (width: number) => void }) {
    const start = (event: PointerEvent) => {
        event.preventDefault();
        const handle = event.currentTarget as HTMLElement;
        handle.setPointerCapture(event.pointerId);
        const origin = event.clientX;
        const initial = props.width;
        const move = (moved: PointerEvent) => {
            const max = Math.max(minWidth, window.innerWidth * 0.75);
            props.onWidth(Math.round(Math.min(max, Math.max(minWidth, initial + origin - moved.clientX))));
        };
        const ends = ["pointerup", "pointercancel", "lostpointercapture"] as const;
        const stop = () => {
            handle.removeEventListener("pointermove", move);
            for (const end of ends) handle.removeEventListener(end, stop);
        };
        handle.addEventListener("pointermove", move);
        for (const end of ends) handle.addEventListener(end, stop);
    };

    return (
        <div
            class="absolute inset-y-0 left-0 z-10 w-1.5 cursor-col-resize transition-colors hover:bg-accent/30"
            role="separator"
            aria-orientation="vertical"
            onPointerDown={start}
        />
    );
}
