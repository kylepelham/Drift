import { resolveModel, savedChoice, type MessageEntry } from "../engine/store";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { debugPanelOpen, setDebugPanelOpen } from "../state/panels";
import { ContextSection, UsageSection } from "./context-meter";
import { selectedSession } from "../state/selection";
import { refreshUsage } from "../state/usage-limits";
import { lightTheme } from "../state/theme";
import { prefsFor } from "../state/prefs";
import { useEngine } from "../engine";
import DOMPurify from "dompurify";
import { t } from "../state/i18n";
import { IconX } from "./icons";

export function DebugPanel() {
    const engine = useEngine();
    const entries = () => engine.state.transcripts[selectedSession() ?? ""] ?? [];

    const provider = () => {
        const id = selectedSession();
        return id
            ? resolveModel(engine.state, prefsFor(id, savedChoice(engine.state, id)).model)?.providerID
            : undefined;
    };

    createEffect(() => {
        const id = provider();
        if (debugPanelOpen() && id) void refreshUsage(id);
    });

    return (
        <Show when={debugPanelOpen() && selectedSession()}>
            <div class="debug-panel flex min-h-0 min-w-0 w-[26rem] shrink-0 flex-col overflow-hidden border-l border-edge bg-surface">
                <div class="flex items-center justify-between border-b border-edge px-3 py-2.5">
                    <span class="text-sm font-semibold text-ink">{t("drift.debug.context")}</span>
                    <button
                        title={t("common.close")}
                        class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        onClick={() => setDebugPanelOpen(false)}
                    >
                        <IconX />
                    </button>
                </div>
                <div class="debug-panel-scroll min-h-0 min-w-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain">
                    <div class="border-b border-edge select-text">
                        <ContextSection sessionId={selectedSession()!} />
                        <Show when={provider()}>{(id) => <UsageSection provider={id()} />}</Show>
                    </div>
                    <For each={entries()}>{(entry) => <DebugRow entry={entry} />}</For>
                </div>
            </div>
        </Show>
    );
}

function DebugRow(props: { entry: MessageEntry }) {
    const [expanded, setExpanded] = createSignal(false);
    const time = () =>
        new Date(props.entry.info.createdAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    return (
        <div class="border-b border-edge/60">
            <button
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors hover:bg-raised/60"
                onClick={() => setExpanded(!expanded())}
            >
                <span
                    class="shrink-0 font-semibold"
                    classList={{
                        "text-accent": props.entry.info.role === "user",
                        "text-ink": props.entry.info.role !== "user",
                    }}
                >
                    {props.entry.info.role}
                </span>
                <span class="min-w-0 flex-1 truncate font-mono text-ink-faint">{props.entry.info.id}</span>
                <span class="shrink-0 text-ink-faint">{time()}</span>
            </button>
            <Show when={expanded()}>
                <div class="max-h-96 overflow-auto border-t border-edge/60 bg-bg px-3 py-2 select-text">
                    <JsonView value={props.entry} />
                </div>
            </Show>
        </div>
    );
}

function JsonView(props: { value: unknown }) {
    const [html, setHtml] = createSignal("");
    const text = createMemo(() => JSON.stringify(props.value, null, 2));
    let generation = 0;
    createEffect(() => {
        const value = text();
        const shikiTheme = lightTheme() ? "github-light" : "github-dark-default";
        const current = ++generation;

        if (value.length > 200_000) return setHtml("");

        void import("shiki").then(async (shiki) => {
            const output = await shiki.codeToHtml(value, { lang: "json", theme: shikiTheme }).catch(() => "");
            if (current === generation) setHtml(DOMPurify.sanitize(output));
        });
    });

    return (
        <Show
            when={html()}
            fallback={
                <pre class="font-mono text-[0.7rem] leading-relaxed whitespace-pre-wrap text-ink-muted">{text()}</pre>
            }
        >
            <div
                class="font-mono text-[0.7rem] leading-relaxed [&_pre]:!bg-transparent [&_pre]:whitespace-pre-wrap"
                // eslint-disable-next-line solid/no-innerhtml -- Shiki output is sanitised with DOMPurify before setHtml.
                innerHTML={html()}
            />
        </Show>
    );
}
