import { computerSwitcher, type ComputerSwitcher, type LinkedComputer } from "../state/computers";
import { createResource, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { ConnectComputer } from "./connect-computer";
import { fixedMenuPosition } from "../state/zoom";
import { MenuSurface } from "./menu-surface";
import { Portal } from "solid-js/web";
import { useEngine } from "../engine";
import { IconCheck } from "./icons";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";

/** The sidebar's brand: on the desktop, the computer this window shows, and a menu to show another. */
export function ComputerBrand() {
    const [switcher] = createResource(computerSwitcher);

    return (
        <Show when={switcher()} fallback={<Brand />}>
            {(switcher) => <Switcher switcher={switcher()} />}
        </Show>
    );
}

function Brand() {
    return (
        <div class="pointer-events-none flex min-w-0 items-center gap-2 text-ink-faint">
            <DriftLogo class="size-4 shrink-0" />
            <span class="truncate text-xs font-medium">Drift</span>
        </div>
    );
}

function Switcher(props: { switcher: ComputerSwitcher }) {
    const engine = useEngine();
    const [computers, { refetch }] = createResource(() => props.switcher.list());
    const [menu, setMenu] = createSignal<{ x: number; y: number }>();
    const [managing, setManaging] = createSignal(false);
    const [switching, setSwitching] = createSignal("");

    const shown = () =>
        computers()?.find((computer) => computer.id === props.switcher.current)?.name ?? t("drift.computers.thisPc");

    async function show(id: string | undefined) {
        setMenu(undefined);
        if (id === props.switcher.current) return;
        setSwitching(id ?? "home");
        try {
            await (id ? props.switcher.open(id) : props.switcher.home());
        } catch (cause) {
            engine.actions.notice({
                title: t("drift.computers.openFailed"),
                message: cause instanceof Error ? cause.message : String(cause),
                variant: "error",
            });
        } finally {
            setSwitching("");
        }
    }

    return (
        <>
            <button
                class="flex min-w-0 items-center gap-2 rounded-md px-1 py-0.5 text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                title={t("drift.computers.switch")}
                aria-haspopup="menu"
                onClick={(event) => {
                    const box = event.currentTarget.getBoundingClientRect();
                    void refetch();
                    setMenu({ x: box.left, y: box.bottom + 4 });
                }}
            >
                <DriftLogo class="size-4 shrink-0" />
                <span class="truncate text-xs font-medium">{switching() ? t("drift.app.connecting") : shown()}</span>
                <svg class="size-3 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="m6 9 6 6 6-6" />
                </svg>
            </button>
            <Show when={menu()}>
                {(at) => (
                    <MenuSurface
                        position={fixedMenuPosition(at().x, at().y, 212, 120 + (computers()?.length ?? 0) * 32)}
                        onClose={() => setMenu(undefined)}
                    >
                        <Choice
                            label={t("drift.computers.thisPc")}
                            chosen={!props.switcher.current}
                            onClick={() => void show(undefined)}
                        />
                        <For each={computers() ?? []}>
                            {(computer) => (
                                <Choice
                                    label={computer.name}
                                    chosen={computer.id === props.switcher.current}
                                    onClick={() => void show(computer.id)}
                                />
                            )}
                        </For>
                        <div class="my-1 border-t border-edge" />
                        <button
                            class="w-full rounded-md px-2 py-1.5 text-left text-sm text-ink-muted transition-colors hover:bg-raised hover:text-ink"
                            onClick={() => {
                                setMenu(undefined);
                                setManaging(true);
                            }}
                        >
                            {t("drift.computers.manage")}
                        </button>
                    </MenuSurface>
                )}
            </Show>
            <Show when={managing()}>
                <ComputersModal
                    switcher={props.switcher}
                    computers={computers() ?? []}
                    onChanged={() => void refetch()}
                    onClose={() => setManaging(false)}
                />
            </Show>
        </>
    );
}

function Choice(props: { label: string; chosen: boolean; onClick: () => void }) {
    return (
        <button
            role="menuitemradio"
            aria-checked={props.chosen}
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-raised hover:text-ink"
            classList={{ "text-ink": props.chosen, "text-ink-muted": !props.chosen }}
            onClick={() => props.onClick()}
        >
            <span class="min-w-0 flex-1 truncate">{props.label}</span>
            <Show when={props.chosen}>
                <IconCheck class="size-3.5 shrink-0" />
            </Show>
        </button>
    );
}

/** Connecting to another computer, and unlinking ones no longer wanted. */
function ComputersModal(props: {
    switcher: ComputerSwitcher;
    computers: LinkedComputer[];
    onChanged: () => void;
    onClose: () => void;
}) {
    let dialog!: HTMLDivElement;
    const [busy, setBusy] = createSignal("");
    const [problem, setProblem] = createSignal("");
    const [confirming, setConfirming] = createSignal<string>();

    onMount(() => onCleanup(activateModal(dialog, props.onClose)));

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

    const opened = (id: string) => {
        const { switcher, onClose } = props;
        return act("open", async () => {
            await switcher.open(id);
            onClose();
        });
    };
    const forget = (computer: LinkedComputer) => {
        const { switcher, onChanged } = props;
        return act(computer.id, async () => {
            await switcher.forget(computer.id);
            setConfirming(undefined);
            onChanged();
        });
    };

    const secondary =
        "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40";

    return (
        <Portal>
            <div
                data-modal-layer
                class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
                onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
            >
                <div
                    ref={dialog}
                    role="dialog"
                    aria-modal="true"
                    aria-label={t("drift.computers.title")}
                    tabIndex={-1}
                    class="fade-up w-[26rem] rounded-xl border border-edge bg-overlay p-4 shadow-2xl shadow-black/40"
                    onClick={(event) => event.stopPropagation()}
                >
                    <div class="mb-1 text-sm font-semibold text-ink">{t("drift.computers.title")}</div>
                    <p class="mb-4 text-xs leading-relaxed text-ink-faint">{t("drift.computers.connectHint")}</p>

                    <Show when={problem()}>
                        <div
                            class="mb-3 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-xs break-words text-danger"
                            role="alert"
                        >
                            {problem()}
                        </div>
                    </Show>

                    <div class="mb-5">
                        <ConnectComputer connector={props.switcher.connector} onLinked={(id) => void opened(id)} />
                    </div>

                    <div class="mb-1 text-[length:calc(0.68rem*var(--ui-text-scale,1))] tracking-wide text-ink-faint uppercase">
                        {t("drift.computers.linked")}
                    </div>
                    <div class="mb-4 space-y-1.5">
                        <For
                            each={props.computers}
                            fallback={<p class="text-xs text-ink-muted">{t("drift.app.empty")}</p>}
                        >
                            {(computer) => (
                                <div class="flex items-center gap-3 rounded-lg border border-edge bg-surface px-3 py-2">
                                    <div class="min-w-0 flex-1">
                                        <div class="truncate text-sm text-ink">{computer.name}</div>
                                        <div class="truncate text-xs text-ink-faint">
                                            {[computer.address, computer.anywhere ? t("drift.app.anywhere") : undefined]
                                                .filter(Boolean)
                                                .join(" · ")}
                                        </div>
                                    </div>
                                    <Show
                                        when={confirming() === computer.id}
                                        fallback={
                                            <button
                                                class={secondary}
                                                disabled={!!busy()}
                                                onClick={() => setConfirming(computer.id)}
                                            >
                                                {t("drift.app.forget")}
                                            </button>
                                        }
                                    >
                                        <button
                                            class={`${secondary} border-danger/40 text-danger`}
                                            disabled={!!busy()}
                                            onClick={() => void forget(computer)}
                                        >
                                            {t("drift.app.forget")}
                                        </button>
                                        <button
                                            class={secondary}
                                            disabled={!!busy()}
                                            onClick={() => setConfirming(undefined)}
                                        >
                                            {t("common.cancel")}
                                        </button>
                                    </Show>
                                </div>
                            )}
                        </For>
                    </div>

                    <div class="flex justify-end">
                        <button
                            class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:text-ink"
                            onClick={() => props.onClose()}
                        >
                            {t("common.close")}
                        </button>
                    </div>
                </div>
            </div>
        </Portal>
    );
}
