import { IconChevronLeft, IconChevronRight, IconGlobe, IconPlus, IconRestore, IconStop, IconX } from "./icons";
import { createEffect, createSignal, For, Show, untrack } from "solid-js";
import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { engineLabel, tabLabel } from "./browser-input";
import { selectedSession } from "../state/selection";
import { rightPanelOpen } from "../state/panels";
import { activePage } from "../engine/browsers";
import { BrowserView } from "./browser-view";
import { SidePanel } from "./side-panel";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    browserPaneCollapsed,
    collapseBrowserPane,
    noteBrowserStart,
    setBrowserPaneShowing,
} from "../state/browser-pane";

import type { BrowserView as ViewSocket } from "../engine/browser-view";
import type { BrowserKind } from "../engine/native/client";
import type { BrowserPage } from "../engine/browsers";

const iconButton =
    "flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink disabled:opacity-40";

/** The selected conversation's browser, right of the chat; it opens when a browser starts and collapses on request. */
export function BrowserPaneHost() {
    const engine = useEngine();
    const narrow = createMediaQuery(smallScreenQuery);

    createEffect(() => {
        for (const record of Object.values(engine.state.browsers)) noteBrowserStart(record.sessionId, record.startedAt);
    });

    // On a phone only one panel fits; the review or debug panel the user opened wins.
    const shown = () => {
        const record = engine.state.browsers[selectedSession() ?? ""];
        if (!record || browserPaneCollapsed(record.sessionId)) return undefined;
        if (narrow() && rightPanelOpen()) return undefined;

        return record.sessionId;
    };
    createEffect(() => setBrowserPaneShowing(!!shown()));

    return (
        <Show when={shown()} keyed>
            {(sessionId) => <BrowserPane sessionId={sessionId} />}
        </Show>
    );
}

function BrowserPane(props: { sessionId: string }) {
    const engine = useEngine();
    const record = () => engine.state.browsers[props.sessionId];
    const active = () => activePage(record());
    let view: ViewSocket | undefined;
    const send: ViewSocket["send"] = (input) => view?.send(input);

    return (
        <SidePanel
            name="browser"
            defaultWidth={760}
            windowControls={!rightPanelOpen()}
            onClose={() => collapseBrowserPane(props.sessionId)}
            title={<Tabs engine={record()?.engine} pages={record()?.pages ?? []} onSend={send} />}
            actions={
                <>
                    <button
                        class={iconButton}
                        title={t("drift.browser.newTab")}
                        onClick={() => send({ type: "newPage" })}
                    >
                        <IconPlus class="size-3.5" />
                    </button>
                    <button
                        class={iconButton}
                        title={t("drift.browser.stop")}
                        onClick={() => void engine.actions.stopBrowser(props.sessionId)}
                    >
                        <IconStop class="size-3.5" />
                    </button>
                </>
            }
        >
            <AddressBar page={active()} onSend={send} />
            <Show when={active()?.dialog}>{(dialog) => <DialogBar dialog={dialog()} onSend={send} />}</Show>
            <BrowserView sessionId={props.sessionId} activePage={active()?.id} onReady={(ready) => (view = ready)} />
        </SidePanel>
    );
}

/** Tabs narrow as a browser's do before the strip scrolls; the wheel scrolls it, as it has no bar. */
function Tabs(props: { engine?: BrowserKind; pages: BrowserPage[]; onSend: ViewSocket["send"] }) {
    return (
        <div
            class="browser-tabs flex min-w-0 flex-[0_1_auto] items-center gap-1 overflow-x-auto"
            onWheel={(event) => {
                if (event.deltaY === 0) return;
                event.currentTarget.scrollLeft += event.deltaY;
            }}
        >
            <span class="flex shrink-0 items-center gap-1 pr-1 text-xs text-ink-faint">
                <IconGlobe class="size-3.5" />
                <Show when={props.engine}>{(engine) => engineLabel(engine())}</Show>
            </span>
            <For each={props.pages}>
                {(page) => (
                    <div
                        class="group flex h-7 w-44 min-w-20 shrink items-center gap-1 rounded-md pr-0.5 pl-2 text-xs transition-colors"
                        classList={{
                            "bg-raised text-ink": page.active,
                            "text-ink-muted hover:bg-raised/60 hover:text-ink": !page.active,
                        }}
                    >
                        <button
                            class="min-w-0 flex-1 truncate text-left"
                            title={page.url}
                            onClick={() => props.onSend({ type: "select", page: page.id })}
                        >
                            {tabLabel(page)}
                        </button>
                        <button
                            class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-hover:opacity-100 hover:text-ink"
                            classList={{ "opacity-100": page.active }}
                            title={t("drift.browser.closeTab")}
                            onClick={() => props.onSend({ type: "close", page: page.id })}
                        >
                            <IconX class="size-3" />
                        </button>
                    </div>
                )}
            </For>
        </div>
    );
}

function AddressBar(props: { page?: BrowserPage; onSend: ViewSocket["send"] }) {
    const [draft, setDraft] = createSignal<string>();
    const go = (url: string) => props.onSend({ type: "navigate", url });

    return (
        <div class="flex h-10 shrink-0 items-center gap-1 border-b border-edge/60 px-2">
            <button
                class={iconButton}
                title={t("drift.browser.back")}
                disabled={!props.page}
                onClick={() => go("back")}
            >
                <IconChevronLeft class="size-3.5" />
            </button>
            <button
                class={iconButton}
                title={t("drift.browser.forward")}
                disabled={!props.page}
                onClick={() => go("forward")}
            >
                <IconChevronRight class="size-3.5" />
            </button>
            <button
                class={iconButton}
                title={t("drift.browser.reload")}
                disabled={!props.page}
                onClick={() => go("reload")}
            >
                <IconRestore class="size-3.5" />
            </button>
            <input
                class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1 font-mono text-xs text-ink outline-none focus:border-edge-strong"
                aria-label={t("drift.browser.address")}
                spellcheck={false}
                value={draft() ?? props.page?.url ?? ""}
                disabled={!props.page}
                onFocus={(event) => event.currentTarget.select()}
                onInput={(event) => setDraft(event.currentTarget.value)}
                onBlur={() => setDraft()}
                onKeyDown={(event) => {
                    if (event.key === "Escape") {
                        setDraft();
                        event.currentTarget.blur();
                    }
                    if (event.key !== "Enter") return;

                    const url = event.currentTarget.value.trim();
                    if (url) go(url);
                    setDraft();
                    event.currentTarget.blur();
                }}
            />
        </div>
    );
}

type Dialog = NonNullable<BrowserPage["dialog"]>;

/** An alert, confirm or prompt on the page; it holds the page until it is answered here or by the agent. */
function DialogBar(props: { dialog: Dialog; onSend: ViewSocket["send"] }) {
    const [answer, setAnswer] = createSignal(untrack(() => props.dialog.defaultPrompt ?? ""));
    const reply = (accept: boolean) =>
        props.onSend({ type: "dialog", accept, text: props.dialog.kind === "prompt" ? answer() : undefined });

    return (
        <div class="flex shrink-0 flex-wrap items-center gap-2 border-b border-edge/60 bg-raised/60 px-3 py-2 text-xs">
            <span class="min-w-0 flex-1 break-words text-ink">{props.dialog.message}</span>
            <Show when={props.dialog.kind === "prompt"}>
                <input
                    class="w-48 rounded-md border border-edge bg-surface px-2 py-1 text-xs text-ink outline-none focus:border-edge-strong"
                    value={answer()}
                    onInput={(event) => setAnswer(event.currentTarget.value)}
                />
            </Show>
            <Show when={props.dialog.kind !== "alert"}>
                <button class="rounded-md px-2.5 py-1 text-ink-muted hover:bg-raised" onClick={() => reply(false)}>
                    {t("common.cancel")}
                </button>
            </Show>
            <button class="rounded-md bg-accent px-2.5 py-1 font-medium text-white" onClick={() => reply(true)}>
                {t("drift.browser.ok")}
            </button>
        </div>
    );
}
