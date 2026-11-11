/** Connecting to another computer's Drift: the computers found on this network, or one typed by its address. The
 * computer's user accepts or declines; meanwhile both screens show a code, which they compare so no other device on
 * the network can stand in between (crates/drift-pairing). The desktop's computers dialog and the app's computers
 * page both use it. */

import { createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { t } from "../state/i18n";

import type { Connector, WaitingComputer } from "../state/computers";

const message = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

export function ConnectComputer(props: { connector: Connector; onLinked: (id: string) => void }) {
    const [nearby, { refetch }] = createResource(() => props.connector.nearby());
    const [address, setAddress] = createSignal("");
    const [asking, setAsking] = createSignal("");
    const [waiting, setWaiting] = createSignal<WaitingComputer>();
    const [problem, setProblem] = createSignal("");
    const found = () => (nearby() ?? []).filter((computer) => !computer.linked);

    async function connect(to: string) {
        if (asking() || waiting()) return;
        const { connector, onLinked } = props;
        setProblem("");
        setAsking(to);
        let request: WaitingComputer;
        try {
            request = await connector.ask(to);
        } catch (cause) {
            setProblem(message(cause));
            return;
        } finally {
            setAsking("");
        }

        setWaiting(request);
        try {
            const linked = await connector.answer(request.id);
            if (waiting()?.id === request.id) onLinked(linked.id);
        } catch (cause) {
            if (waiting()?.id === request.id) setProblem(message(cause));
        } finally {
            if (waiting()?.id === request.id) setWaiting(undefined);
        }
    }

    function cancel() {
        const request = waiting();
        if (!request) return;
        setWaiting(undefined);
        void props.connector.cancel(request.id).catch(() => undefined);
    }
    onCleanup(cancel);

    const secondary =
        "h-8 shrink-0 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40";
    const label = "text-[length:calc(0.68rem*var(--ui-text-scale,1))] tracking-wide text-ink-faint uppercase";

    return (
        <div class="connect-computer space-y-3">
            <Show when={problem()}>
                <div
                    class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-xs break-words text-danger"
                    role="alert"
                >
                    {problem()}
                </div>
            </Show>

            <Show
                when={waiting()}
                fallback={
                    <>
                        <div class="flex items-center justify-between gap-2">
                            <span class={label}>{t("drift.connect.nearby")}</span>
                            <button
                                class="text-xs text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
                                disabled={nearby.loading}
                                onClick={() => void refetch()}
                            >
                                {t("drift.connect.lookAgain")}
                            </button>
                        </div>
                        <Show
                            when={!nearby.loading}
                            fallback={
                                <p class="text-xs text-ink-muted" role="status">
                                    {t("drift.connect.looking")}
                                </p>
                            }
                        >
                            <Show when={nearby.error}>
                                <p class="text-xs break-words text-danger">{message(nearby.error)}</p>
                            </Show>
                            <div class="space-y-1.5">
                                <For
                                    each={found()}
                                    fallback={
                                        <Show when={!nearby.error}>
                                            <p class="text-xs leading-relaxed text-ink-muted">
                                                {t("drift.connect.none")}
                                            </p>
                                        </Show>
                                    }
                                >
                                    {(computer) => (
                                        <button
                                            class="flex w-full items-center gap-3 rounded-lg border border-edge bg-surface px-3 py-2 text-left transition-colors hover:border-edge-strong disabled:opacity-60"
                                            disabled={!!asking()}
                                            onClick={() => void connect(computer.address)}
                                        >
                                            <div class="min-w-0 flex-1">
                                                <div class="truncate text-sm text-ink">{computer.name}</div>
                                                <div class="truncate text-xs text-ink-faint">
                                                    {asking() === computer.address
                                                        ? t("drift.app.connecting")
                                                        : computer.address}
                                                </div>
                                            </div>
                                        </button>
                                    )}
                                </For>
                            </div>
                        </Show>

                        <label class="block">
                            <span class={`mb-1 block ${label}`}>{t("drift.connect.address")}</span>
                            <div class="flex gap-2">
                                <input
                                    class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 text-xs outline-none focus:border-edge-strong"
                                    placeholder={t("drift.connect.addressPlaceholder")}
                                    spellcheck={false}
                                    autocapitalize="off"
                                    value={address()}
                                    onInput={(event) => setAddress(event.currentTarget.value)}
                                    onKeyDown={(event) =>
                                        event.key === "Enter" && address().trim() && void connect(address().trim())
                                    }
                                />
                                <button
                                    class="h-8 shrink-0 rounded-md bg-accent px-3 text-xs font-medium text-accent-ink disabled:opacity-40"
                                    disabled={!address().trim() || !!asking()}
                                    onClick={() => void connect(address().trim())}
                                >
                                    {asking() && asking() === address().trim()
                                        ? t("drift.app.connecting")
                                        : t("drift.connect.connect")}
                                </button>
                            </div>
                        </label>
                    </>
                }
            >
                {(request) => (
                    <div class="space-y-3 rounded-lg border border-edge bg-surface px-4 py-4 text-center" role="status">
                        <p class="text-sm text-ink">{t("drift.connect.waiting", { name: request().name })}</p>
                        <p class="font-mono text-3xl font-semibold tracking-[0.2em] text-ink">{request().code}</p>
                        <p class="text-xs leading-relaxed text-ink-faint">
                            {t("drift.connect.compare", { name: request().name })}
                        </p>
                        <button class={secondary} onClick={cancel}>
                            {t("common.cancel")}
                        </button>
                    </div>
                )}
            </Show>
        </div>
    );
}
