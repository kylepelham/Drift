import { createEffect, onCleanup, onMount, type JSX } from "solid-js";
import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { createDismissOnOutside } from "./dismiss";
import { Portal } from "solid-js/web";
import { t } from "../state/i18n";

export function MenuSurface(props: {
    position: { left: number; top: number };
    onClose: () => void;
    children: JSX.Element;
}) {
    let panel!: HTMLDivElement;
    const narrow = createMediaQuery(smallScreenQuery);
    createEffect(() => {
        if (narrow()) onCleanup(activateModal(panel, props.onClose));
    });
    createDismissOnOutside({
        enabled: () => !narrow(),
        inside: () => [panel],
        onDismiss: () => props.onClose(),
        escape: true,
    });
    // Opened from a row by keyboard or pointer alike, the menu takes focus and gives it back when it closes.
    const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
    onMount(() => queueMicrotask(() => menuItems(panel)[0]?.focus({ preventScroll: true })));
    onCleanup(() => {
        if (!returnFocus?.isConnected) return;
        queueMicrotask(() => {
            if (document.activeElement === document.body || !document.activeElement)
                returnFocus.focus({ preventScroll: true });
        });
    });
    return (
        <Portal>
            <div
                data-modal-layer={narrow() ? "" : undefined}
                classList={{ "fixed inset-0 z-50 bg-black/50": narrow() }}
                onPointerDown={(event) => {
                    if (narrow()) closeOnBackdropPointerDown(event, props.onClose, panel);
                }}
            >
                <div
                    ref={panel}
                    role={narrow() ? "dialog" : "menu"}
                    aria-modal={narrow() ? true : undefined}
                    aria-label={t("common.moreOptions")}
                    tabIndex={-1}
                    class="workspace-menu fade-up fixed z-40 max-h-80 w-52 overflow-y-auto rounded-lg border border-edge bg-overlay p-1.5 shadow-xl shadow-black/40"
                    style={narrow() ? undefined : { left: `${props.position.left}px`, top: `${props.position.top}px` }}
                    onKeyDown={(event) => moveInMenu(event, panel)}
                >
                    {props.children}
                </div>
            </div>
        </Portal>
    );
}

function menuItems(panel: HTMLElement) {
    return [...panel.querySelectorAll<HTMLElement>("button:not([disabled])")].filter(
        (item) => item.getClientRects().length > 0,
    );
}

/** Up and Down walk the menu's items, wrapping; Home and End jump to either end. */
function moveInMenu(event: KeyboardEvent, panel: HTMLElement) {
    const items = menuItems(panel);
    const index = items.indexOf(document.activeElement as HTMLElement);
    const targets: Record<string, number> = {
        ArrowDown: (index + 1) % items.length,
        ArrowUp: index <= 0 ? items.length - 1 : index - 1,
        Home: 0,
        End: items.length - 1,
    };
    const next = targets[event.key];
    if (next === undefined || !items.length) return;
    event.preventDefault();
    items[next]?.focus();
}
