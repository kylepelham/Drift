import { createResource, createSignal, For, Show } from "solid-js";
import { appApi, openDrift, type Computer } from "./api";
import { t } from "../state/i18n";

const primary =
    "h-11 w-full rounded-lg bg-accent px-4 text-sm font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40";
const secondary =
    "h-9 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40";

/** The app's home when no PC is open: the linked PCs, and scanning a pairing code to link another. */
export function Computers() {
    const [computers, { refetch }] = createResource(appApi.computers);
    const [busy, setBusy] = createSignal("");
    const [problem, setProblem] = createSignal(new URLSearchParams(window.location.search).get("problem") ?? "");

    /** Runs one action at a time; its failure is shown above the list. */
    async function act(name: string, action: () => Promise<unknown>) {
        if (busy()) return;
        setBusy(name);
        setProblem("");
        try {
            await action();
        } catch (cause) {
            setProblem(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setBusy("");
        }
    }

    const open = (computer: Computer) =>
        act(computer.id, async () => {
            await appApi.use(computer.id);
            openDrift();
        });
    const forget = (computer: Computer) =>
        act(`forget-${computer.id}`, async () => {
            await appApi.forget(computer.id);
            await refetch();
        });
    const scan = () =>
        act("scan", async () => {
            if (await appApi.scan()) openDrift();
        });

    return (
        <main class="app-computers mx-auto flex min-h-full max-w-lg flex-col gap-5 bg-bg px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-ink">
            <h1 class="text-lg font-semibold">{t("drift.app.title")}</h1>

            <Show when={problem()}>
                <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger" role="alert">
                    {problem()}
                </div>
            </Show>
            <Show when={computers.error}>
                <div class="rounded-lg border border-danger/40 px-3 py-2 text-sm" role="alert">
                    <p class="break-words text-danger">{String(computers.error)}</p>
                    <button class={secondary} onClick={() => void refetch()}>
                        {t("drift.preview.retry")}
                    </button>
                </div>
            </Show>
            <Show when={computers.loading}>
                <p role="status" class="text-sm text-ink-muted">
                    {t("common.loading")}
                </p>
            </Show>

            <Show when={computers() && !computers.error}>
                <Linked
                    computers={computers() ?? []}
                    busy={busy()}
                    onOpen={(computer) => void open(computer)}
                    onForget={(computer) => void forget(computer)}
                />
            </Show>

            <section class="space-y-3">
                <button class={primary} disabled={!!busy()} onClick={() => void scan()}>
                    {busy() === "scan" ? t("drift.app.scanning") : t("drift.app.scan")}
                </button>
                <p class="text-xs leading-relaxed text-ink-faint">{t("drift.app.scanHint")}</p>
            </section>
        </main>
    );
}

function Linked(props: {
    computers: Computer[];
    busy: string;
    onOpen: (computer: Computer) => void;
    onForget: (computer: Computer) => void;
}) {
    const [confirming, setConfirming] = createSignal<string>();
    const where = (computer: Computer) =>
        [computer.address, computer.anywhere ? t("drift.app.anywhere") : undefined].filter(Boolean).join(" · ");

    return (
        <section class="space-y-2">
            <For each={props.computers} fallback={<p class="text-sm text-ink-muted">{t("drift.app.empty")}</p>}>
                {(computer) => (
                    <div class="flex items-center gap-3 rounded-lg border border-edge bg-surface px-3 py-2.5">
                        <button
                            class="min-w-0 flex-1 text-left disabled:opacity-60"
                            disabled={!!props.busy}
                            onClick={() => props.onOpen(computer)}
                        >
                            <div class="truncate text-sm font-medium">{computer.name}</div>
                            <div class="truncate text-xs text-ink-faint">
                                {props.busy === computer.id ? t("drift.app.connecting") : where(computer)}
                            </div>
                        </button>
                        <Show
                            when={confirming() === computer.id}
                            fallback={
                                <button
                                    class={secondary}
                                    disabled={!!props.busy}
                                    onClick={() => setConfirming(computer.id)}
                                >
                                    {t("drift.app.forget")}
                                </button>
                            }
                        >
                            <div class="flex shrink-0 flex-col gap-1">
                                <button
                                    class={`${secondary} border-danger/40 text-danger`}
                                    disabled={!!props.busy}
                                    onClick={() => props.onForget(computer)}
                                >
                                    {t("drift.app.forget")}
                                </button>
                                <button
                                    class={secondary}
                                    disabled={!!props.busy}
                                    onClick={() => setConfirming(undefined)}
                                >
                                    {t("common.cancel")}
                                </button>
                            </div>
                        </Show>
                    </div>
                )}
            </For>
        </section>
    );
}
