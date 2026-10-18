import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { createEffect, onCleanup, type JSX } from "solid-js";
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
                >
                    {props.children}
                </div>
            </div>
        </Portal>
    );
}
