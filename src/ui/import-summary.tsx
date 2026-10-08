import { dismissImportSummary, importSummary, type ImportSummary } from "../state/opencode-import";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { t } from "../state/i18n";
import { IconX } from "./icons";

/** What the one-time import from opencode brought in and left behind; shown once, then gone. */
export function ImportSummaryHost() {
    return <Show when={importSummary()}>{(summary) => <ImportSummaryDialog summary={summary()} />}</Show>;
}

function ImportSummaryDialog(props: { summary: ImportSummary }) {
    let dialog!: HTMLDivElement;
    onMount(() => onCleanup(activateModal(dialog, dismissImportSummary)));
    const s = () => props.summary;
    const waiting = () => Object.entries(s().waiting).sort((a, b) => b[1] - a[1]);
    const left = () => s().leftOut;
    const on = () => s().servers.length - s().serversOff.length;
    return (
        <Portal>
            <div
                data-modal-layer
                class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
                onPointerDown={(event) => closeOnBackdropPointerDown(event, dismissImportSummary, dialog)}
            >
                <div
                    ref={dialog}
                    role="dialog"
                    aria-modal="true"
                    aria-label={t("drift.import.done.title")}
                    tabIndex={-1}
                    class="fade-up flex max-h-[70vh] w-[32rem] flex-col overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40"
                    onClick={(event) => event.stopPropagation()}
                >
                    <div class="flex items-center justify-between border-b border-edge px-4 py-3">
                        <div class="text-sm font-semibold text-ink">{t("drift.import.done.title")}</div>
                        <button
                            title={t("common.close")}
                            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                            onClick={dismissImportSummary}
                        >
                            <IconX />
                        </button>
                    </div>
                    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm text-ink">
                        <ul class="space-y-1">
                            <Show when={s().conversations > 0}>
                                <li>
                                    {t("drift.import.done.conversations", {
                                        count: s().conversations.toLocaleString(),
                                    })}
                                </li>
                            </Show>
                            <Show when={s().undoable > 0}>
                                <li class="text-ink-muted">
                                    {t("drift.import.done.undoable", { count: s().undoable.toLocaleString() })}
                                </li>
                            </Show>
                            <Show when={s().signIns.length > 0}>
                                <li>{t("drift.import.done.signIns", { names: s().signIns.join(", ") })}</li>
                            </Show>
                            <Show when={s().servers.length > 0}>
                                <li>{t("drift.import.done.servers", { count: s().servers.length, on: on() })}</li>
                            </Show>
                            <Show when={s().files > 0}>
                                <li>{t("drift.import.done.files", { count: s().files })}</li>
                            </Show>
                        </ul>
                        <Group
                            title={t("drift.import.done.waiting")}
                            items={waiting().map(([directory, count]) => `${directory} (${count})`)}
                        />
                        <Group title={t("drift.import.done.pending")} items={s().pending} />
                        <Group title={t("drift.import.done.leftOut.signIns")} items={left().signIns} />
                        <Group title={t("drift.import.done.leftOut.plugins")} items={left().plugins} />
                        <Group title={t("drift.import.done.leftOut.settings")} items={left().settings} />
                        <Group title={t("drift.import.done.leftOut.servers")} items={left().servers} />
                        <Group
                            title={t("drift.import.done.leftOut.failed")}
                            items={[
                                ...left().failed,
                                ...(s().failed > 0
                                    ? [t("drift.import.done.conversations", { count: s().failed.toLocaleString() })]
                                    : []),
                            ]}
                        />
                    </div>
                    <div class="flex justify-end border-t border-edge px-4 py-3">
                        <button
                            class="h-8 rounded-md bg-accent px-3.5 text-xs font-medium text-accent-ink transition-colors hover:brightness-105"
                            onClick={dismissImportSummary}
                        >
                            {t("drift.import.done.close")}
                        </button>
                    </div>
                </div>
            </div>
        </Portal>
    );
}

function Group(props: { title: string; items: string[] }): JSX.Element {
    return (
        <Show when={props.items.length > 0}>
            <div>
                <div class="mb-1 text-xs font-medium text-ink-muted">{props.title}</div>
                <ul class="space-y-0.5 text-xs text-ink-faint">
                    <For each={props.items}>
                        {(item) => (
                            <li class="truncate" title={item}>
                                {item}
                            </li>
                        )}
                    </For>
                </ul>
            </div>
        </Show>
    );
}
