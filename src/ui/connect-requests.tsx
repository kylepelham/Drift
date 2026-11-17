/** Asks this PC's user about each device asking to connect, wherever the window is: it shows the device's name, where
 * it is, and the code its own screen shows, which the user compares before accepting (crates/drift-pairing). Closing
 * the prompt leaves the request waiting; it runs out after two minutes. */

import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { activateModal } from "./modal";
import { Portal } from "solid-js/web";
import { t } from "../state/i18n";
import {
    answerConnectRequest,
    connectRequests,
    listenConnectRequests,
    type ConnectRequest,
} from "../state/remote-access";

export function ConnectRequestHost() {
    const [requests, setRequests] = createSignal<ConnectRequest[]>([]);
    const [dismissed, setDismissed] = createSignal<string[]>([]);
    const refresh = () => void connectRequests().then(setRequests, () => setRequests([]));

    onMount(() => {
        refresh();
        const stop = listenConnectRequests(refresh);
        onCleanup(() => void stop?.then((release) => release()));
    });

    const shown = () => requests().find((request) => !dismissed().includes(request.id));

    return (
        <Show when={shown()} keyed>
            {(request) => (
                <Prompt
                    request={request}
                    onAnswered={refresh}
                    onDismiss={() => setDismissed((ids) => [...ids, request.id])}
                />
            )}
        </Show>
    );
}

function Prompt(props: { request: ConnectRequest; onAnswered: () => void; onDismiss: () => void }) {
    let dialog!: HTMLDivElement;
    const [busy, setBusy] = createSignal(false);
    const [problem, setProblem] = createSignal("");
    onMount(() => onCleanup(activateModal(dialog, props.onDismiss)));

    async function answer(accept: boolean) {
        if (busy()) return;
        const { request, onAnswered } = props;
        setBusy(true);
        setProblem("");
        try {
            await answerConnectRequest(request.id, accept);
            onAnswered();
        } catch (cause) {
            setProblem(cause instanceof Error ? cause.message : String(cause));
            onAnswered();
        } finally {
            setBusy(false);
        }
    }

    return (
        <Portal>
            <div data-modal-layer class="fixed inset-0 z-40 flex items-center justify-center bg-black/50">
                <div
                    ref={dialog}
                    role="alertdialog"
                    aria-modal="true"
                    aria-label={t("drift.remote.request.title", { name: props.request.name })}
                    tabIndex={-1}
                    class="connect-request fade-up w-[24rem] rounded-xl border border-edge bg-overlay p-5 text-center shadow-2xl shadow-black/40"
                >
                    <div class="text-sm font-semibold text-ink">
                        {t("drift.remote.request.title", { name: props.request.name })}
                    </div>
                    <div class="mt-1 text-xs text-ink-faint">
                        {t("drift.remote.request.from", { address: props.request.address })}
                    </div>
                    <div class="my-4 font-mono text-3xl font-semibold tracking-[0.2em] text-ink">
                        {props.request.code}
                    </div>
                    <p class="text-xs leading-relaxed text-ink-muted">
                        {t("drift.remote.request.compare", { name: props.request.name })}
                    </p>
                    <Show when={problem()}>
                        <p class="mt-3 text-xs break-words text-danger" role="alert">
                            {problem()}
                        </p>
                    </Show>
                    <div class="mt-5 flex justify-center gap-2">
                        <button
                            class="h-9 min-w-24 rounded-md border border-edge-strong px-4 text-xs font-medium text-ink-muted transition-colors hover:text-ink disabled:opacity-40"
                            disabled={busy()}
                            onClick={() => void answer(false)}
                        >
                            {t("drift.remote.request.decline")}
                        </button>
                        <button
                            class="h-9 min-w-24 rounded-md bg-accent px-4 text-xs font-medium text-accent-ink transition-[filter] hover:brightness-110 disabled:opacity-40"
                            disabled={busy()}
                            onClick={() => void answer(true)}
                        >
                            {t("drift.remote.request.accept")}
                        </button>
                    </div>
                </div>
            </div>
        </Portal>
    );
}
