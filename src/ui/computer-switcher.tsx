import { createResource, createSignal, For, onCleanup, onMount, Show, type Accessor } from "solid-js";
import { computerSwitcher, type ComputerSwitcher, type LinkedComputer } from "../state/computers";
import { computerDigit, formatCombo, onKeybind } from "../state/keybinds";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { ConnectComputer } from "./connect-computer";
import { fixedMenuPosition } from "../state/zoom";
import { MenuSurface } from "./menu-surface";
import { Portal } from "solid-js/web";
import { useEngine } from "../engine";
import { IconCheck } from "./icons";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";

/** How often the tabs ask which linked computers answer. */
const ONLINE_EVERY_MS = 60_000;

/** One computer switching state for the sidebar's brand menu, its tabs and the shortcuts. Undefined where there is no
 * PC to switch from: a browser, or the phone app. */
export type Switching = {
    switcher: ComputerSwitcher;
    computers: () => LinkedComputer[];
    tabs: () => boolean;
    /** The id being opened, "home" while going back to this PC, or empty. */
    switching: () => string;
    refetch: () => void;
    /** Shows a linked computer, or this PC for undefined. */
    show(id: string | undefined): Promise<void>;
    setTabs(on: boolean): Promise<void>;
};

export function createSwitching(): Accessor<Switching | undefined> {
    const engine = useEngine();
    const [switcher] = createResource(computerSwitcher);
    const [listed, { refetch, mutate }] = createResource(switcher, (switcher) => switcher.list());
    const [switching, setSwitching] = createSignal("");
    const failed = (cause: unknown) =>
        engine.actions.notice({
            title: t("drift.computers.openFailed"),
            message: cause instanceof Error ? cause.message : String(cause),
            variant: "error",
        });

    const state = (switcher: ComputerSwitcher): Switching => ({
        switcher,
        computers: () => listed()?.computers ?? [],
        tabs: () => listed()?.tabs ?? false,
        switching,
        refetch: () => void refetch(),
        async show(id) {
            if (id === switcher.current || switching()) return;
            setSwitching(id ?? "home");
            try {
                await (id ? switcher.open(id) : switcher.home());
            } catch (cause) {
                failed(cause);
            } finally {
                setSwitching("");
            }
        },
        async setTabs(on) {
            const before = listed();
            if (before) mutate({ ...before, tabs: on });
            await switcher.setTabs(on).catch((cause: unknown) => {
                if (before) mutate(before);
                failed(cause);
            });
        },
    });
    const current = () => {
        const found = switcher();
        return found ? state(found) : undefined;
    };

    // This PC, then the linked computers in the menu's order.
    const order = (switching: Switching) => [undefined, ...switching.computers().map((computer) => computer.id)];
    const step = (by: number) => {
        const switching = current();
        if (!switching) return;
        const ids = order(switching);
        const at = ids.indexOf(switching.switcher.current);
        void switching.show(ids[(at + by + ids.length) % ids.length]);
    };
    onKeybind("nextComputer", () => step(1));
    onKeybind("previousComputer", () => step(-1));
    const byDigit = (event: KeyboardEvent) => {
        const digit = computerDigit(event);
        const switching = current();
        if (!digit || !switching) return;
        const ids = order(switching);
        if (digit > ids.length) return;
        event.preventDefault();
        void switching.show(ids[digit - 1]);
    };
    document.addEventListener("keydown", byDigit);
    onCleanup(() => document.removeEventListener("keydown", byDigit));

    return current;
}

/** The shortcut that opens the computer at `place` in the menu, This PC being 1; none past 9. */
function placeShortcut(place: number) {
    return place <= 9 ? formatCombo(`ctrl+alt+${place}`) : undefined;
}

/** The sidebar's brand: on the desktop, the computer this window shows, and a menu to show another. */
export function ComputerBrand(props: { switching: Switching | undefined }) {
    return (
        <Show when={props.switching} fallback={<Brand />}>
            {(switching) => <Switcher switching={switching()} />}
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

function Switcher(props: { switching: Switching }) {
    const [menu, setMenu] = createSignal<{ x: number; y: number }>();
    const [managing, setManaging] = createSignal(false);
    const switcher = () => props.switching.switcher;
    const computers = () => props.switching.computers();

    const shown = () =>
        computers().find((computer) => computer.id === switcher().current)?.name ?? t("drift.computers.thisPc");
    const choose = (id: string | undefined) => {
        setMenu(undefined);
        void props.switching.show(id);
    };

    return (
        <>
            <button
                class="flex min-w-0 items-center gap-2 rounded-md px-1 py-0.5 text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                title={t("drift.computers.switch")}
                aria-haspopup="menu"
                onClick={(event) => {
                    const box = event.currentTarget.getBoundingClientRect();
                    props.switching.refetch();
                    setMenu({ x: box.left, y: box.bottom + 4 });
                }}
            >
                <DriftLogo class="size-4 shrink-0" />
                <span class="truncate text-xs font-medium">
                    {props.switching.switching() ? t("drift.app.connecting") : shown()}
                </span>
                <svg class="size-3 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="m6 9 6 6 6-6" />
                </svg>
            </button>
            <Show when={menu()}>
                {(at) => (
                    <MenuSurface
                        position={fixedMenuPosition(at().x, at().y, 240, 152 + computers().length * 32)}
                        onClose={() => setMenu(undefined)}
                    >
                        <Choice
                            label={t("drift.computers.thisPc")}
                            shortcut={placeShortcut(1)}
                            chosen={!switcher().current}
                            onClick={() => choose(undefined)}
                        />
                        <For each={computers()}>
                            {(computer, index) => (
                                <Choice
                                    label={computer.name}
                                    shortcut={placeShortcut(index() + 2)}
                                    chosen={computer.id === switcher().current}
                                    onClick={() => choose(computer.id)}
                                />
                            )}
                        </For>
                        <div class="my-1 border-t border-edge" />
                        <button
                            role="menuitemcheckbox"
                            aria-checked={props.switching.tabs()}
                            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-ink-muted transition-colors hover:bg-raised hover:text-ink"
                            onClick={() => void props.switching.setTabs(!props.switching.tabs())}
                        >
                            <span class="min-w-0 flex-1 truncate">{t("drift.computers.tabs")}</span>
                            <Show when={props.switching.tabs()}>
                                <IconCheck class="size-3.5 shrink-0" />
                            </Show>
                        </button>
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
                    switcher={switcher()}
                    computers={computers()}
                    onChanged={props.switching.refetch}
                    onClose={() => setManaging(false)}
                />
            </Show>
        </>
    );
}

function Choice(props: { label: string; shortcut?: string; chosen: boolean; onClick: () => void }) {
    return (
        <button
            role="menuitemradio"
            aria-checked={props.chosen}
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-raised hover:text-ink"
            classList={{ "text-ink": props.chosen, "text-ink-muted": !props.chosen }}
            onClick={() => props.onClick()}
        >
            <span class="min-w-0 flex-1 truncate">{props.label}</span>
            <Show when={props.shortcut}>
                <span class="shrink-0 text-[length:calc(0.68rem*var(--ui-text-scale,1))] text-ink-faint">
                    {props.shortcut}
                </span>
            </Show>
            <span class="flex size-3.5 shrink-0 items-center">
                <Show when={props.chosen}>
                    <IconCheck class="size-3.5" />
                </Show>
            </span>
        </button>
    );
}

/** This PC and each linked computer as one-click tabs under the sidebar's top row, once turned on in the menu. A
 * computer that does not answer is dimmed; opening it still tries, and says why when it cannot. */
export function ComputerTabs(props: { switching: Switching | undefined }) {
    return (
        <Show when={props.switching?.tabs() && props.switching}>{(switching) => <Tabs switching={switching()} />}</Show>
    );
}

function Tabs(props: { switching: Switching }) {
    const [online, setOnline] = createSignal<string[]>();
    const check = () => void props.switching.switcher.online().then(setOnline, () => setOnline(undefined));
    onMount(() => {
        check();
        const timer = setInterval(check, ONLINE_EVERY_MS);
        onCleanup(() => clearInterval(timer));
    });

    const current = () => props.switching.switcher.current;
    const tab = "max-w-36 shrink-0 truncate rounded-md px-2 py-1 text-xs transition-colors disabled:cursor-default";

    return (
        <div class="computer-tabs flex shrink-0 gap-1 overflow-x-auto px-3 pb-2" role="tablist">
            <button
                role="tab"
                aria-selected={!current()}
                class={tab}
                classList={{
                    "bg-raised text-ink": !current(),
                    "text-ink-muted hover:bg-raised/60 hover:text-ink": !!current(),
                }}
                title={placeShortcut(1)}
                onClick={() => void props.switching.show(undefined)}
            >
                {t("drift.computers.thisPc")}
            </button>
            <For each={props.switching.computers()}>
                {(computer, index) => {
                    const offline = () => !!online() && !online()!.includes(computer.id);
                    return (
                        <button
                            role="tab"
                            aria-selected={computer.id === current()}
                            class={tab}
                            classList={{
                                "bg-raised text-ink": computer.id === current(),
                                "text-ink-muted hover:bg-raised/60 hover:text-ink": computer.id !== current(),
                                "opacity-45": offline() && computer.id !== current(),
                            }}
                            title={[offline() ? t("drift.computers.offline") : "", placeShortcut(index() + 2) ?? ""]
                                .filter(Boolean)
                                .join(" · ")}
                            onClick={() => void props.switching.show(computer.id)}
                        >
                            {props.switching.switching() === computer.id ? t("drift.app.connecting") : computer.name}
                        </button>
                    );
                }}
            </For>
        </div>
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
