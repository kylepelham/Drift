import { appApi, openDrift, type Computer, type Found } from "./api";
import { createResource, createSignal, For, Show } from "solid-js";
import { LinkByCode } from "./link-by-code";
import { t } from "../state/i18n";

const primary =
    "h-11 w-full rounded-lg bg-accent px-4 text-sm font-medium text-accent-ink transition-colors hover:brightness-105 disabled:opacity-40";
const secondary =
    "h-9 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40";

/** The app's home when no PC is open: the linked PCs, and the ways to link another. */
export function Computers() {
    const [computers, { refetch }] = createResource(appApi.computers);
    const [found, setFound] = createSignal<Found[] | null>(null);
    const [linking, setLinking] = createSignal<{ address: string; port?: number } | null>(null);
    const [busy, setBusy] = createSignal("");
    const [problem, setProblem] = createSignal(new URLSearchParams(window.location.search).get("problem") ?? "");
    const [address, setAddress] = createSignal("");

    /** Runs one action at a time; its failure is shown above the list. */
    async function act(name: string, action: () => Promise<unknown>) {
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
    const discover = () => act("discover", async () => setFound(await appApi.discover()));

    return (
        <main class="mx-auto flex min-h-full max-w-lg flex-col gap-5 bg-bg px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-ink">
            <h1 class="text-lg font-semibold">{t("drift.app.title")}</h1>

            <Show when={problem()}>
                <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm text-danger" role="alert">
                    {problem()}
                </div>
            </Show>

            <Show
                when={linking()}
                fallback={
                    <>
                        <Linked
                            computers={computers() ?? []}
                            busy={busy()}
                            onOpen={(computer) => void open(computer)}
                            onForget={(computer) => void forget(computer)}
                        />

                        <section class="space-y-3">
                            <button class={primary} disabled={!!busy()} onClick={() => void scan()}>
                                {busy() === "scan" ? t("drift.app.scanning") : t("drift.app.scan")}
                            </button>
                            <p class="text-xs leading-relaxed text-ink-faint">{t("drift.app.scanHint")}</p>
                        </section>

                        <Nearby
                            found={found()}
                            busy={busy() === "discover"}
                            onDiscover={() => void discover()}
                            onLink={(entry) => setLinking({ address: entry.found.address, port: entry.found.port })}
                        />

                        <section class="space-y-2">
                            <div class="text-xs text-ink-muted">{t("drift.app.byAddress")}</div>
                            <form
                                class="flex gap-2"
                                onSubmit={(event) => {
                                    event.preventDefault();
                                    if (address().trim()) setLinking({ address: address().trim() });
                                }}
                            >
                                <input
                                    class="h-9 min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 font-mono text-sm outline-none focus:border-edge-strong"
                                    inputmode="decimal"
                                    placeholder="192.168.1.20"
                                    value={address()}
                                    onInput={(event) => setAddress(event.currentTarget.value)}
                                />
                                <button type="submit" class={secondary} disabled={!address().trim()}>
                                    {t("drift.app.linkWithCode")}
                                </button>
                            </form>
                        </section>
                    </>
                }
            >
                {(target) => (
                    <LinkByCode
                        address={target().address}
                        port={target().port}
                        onCancel={() => setLinking(null)}
                        onLinked={openDrift}
                    />
                )}
            </Show>
        </main>
    );
}

function Linked(props: {
    computers: Computer[];
    busy: string;
    onOpen: (computer: Computer) => void;
    onForget: (computer: Computer) => void;
}) {
    return (
        <Show when={props.computers.length > 0}>
            <section class="space-y-2">
                <For each={props.computers}>
                    {(computer) => (
                        <div class="flex items-center gap-3 rounded-lg border border-edge bg-surface px-3 py-2.5">
                            <button
                                class="min-w-0 flex-1 text-left disabled:opacity-60"
                                disabled={!!props.busy}
                                onClick={() => props.onOpen(computer)}
                            >
                                <div class="truncate text-sm font-medium">{computer.name}</div>
                                <div class="truncate font-mono text-xs text-ink-faint">
                                    {props.busy === computer.id ? t("drift.app.connecting") : computer.address}
                                </div>
                            </button>
                            <button class={secondary} disabled={!!props.busy} onClick={() => props.onForget(computer)}>
                                {t("drift.app.forget")}
                            </button>
                        </div>
                    )}
                </For>
            </section>
        </Show>
    );
}

/** PCs answering on this network; the ones already linked are left out. */
function Nearby(props: {
    found: Found[] | null;
    busy: boolean;
    onDiscover: () => void;
    onLink: (entry: Found) => void;
}) {
    const unlinked = () => (props.found ?? []).filter((entry) => !entry.linked);

    return (
        <section class="space-y-2">
            <button class={secondary} disabled={props.busy} onClick={() => props.onDiscover()}>
                {props.busy ? t("drift.app.searching") : t("drift.app.discover")}
            </button>
            <Show when={props.found && unlinked().length === 0}>
                <p class="text-xs text-ink-faint">{t("drift.app.noneFound")}</p>
            </Show>
            <For each={unlinked()}>
                {(entry) => (
                    <div class="flex items-center gap-3 rounded-lg border border-edge px-3 py-2">
                        <div class="min-w-0 flex-1">
                            <div class="truncate text-sm">{entry.found.name}</div>
                            <div class="truncate font-mono text-xs text-ink-faint">{entry.found.address}</div>
                        </div>
                        <button class={secondary} onClick={() => props.onLink(entry)}>
                            {t("drift.app.linkWithCode")}
                        </button>
                    </div>
                )}
            </For>
        </section>
    );
}
