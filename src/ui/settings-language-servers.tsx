import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { activeWorkspace } from "../state/workspaces";
import { SettingsGroup } from "./settings-controls";
import { IconArrowUpRight } from "./icons";
import { Badge, Toggle } from "./controls";
import { openExternal } from "../shell";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { LspStatus } from "../engine/native/client";

type Group = "installed" | "missing" | "off";

function groupOf(server: LspStatus): Group {
    if (server.state === "off" || server.state === "missing") return server.state;
    return "installed";
}

const button =
    "rounded-md border border-edge px-2 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40";

/** Every language server Drift could use: whether it is installed and answers, and how to get one that is missing. */
export function LanguageServersSection() {
    const engine = useEngine();
    const here = () => activeWorkspace()?.path;
    const [servers, setServers] = createSignal<LspStatus[]>([]);
    const [loading, setLoading] = createSignal(true);
    const [busy, setBusy] = createSignal("");
    const [failures, setFailures] = createSignal<Record<string, string>>({});
    const [message, setMessage] = createSignal("");

    const load = async () => {
        setLoading(true);
        try {
            setServers(await engine.actions.languageServers(here()));
        } catch (cause) {
            setMessage(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setLoading(false);
        }
    };
    onMount(() => void load());

    /** Runs one server's action, keeping its failure beside it rather than in a banner. */
    const act = async (server: LspStatus, work: () => Promise<LspStatus[]>, done?: string) => {
        setBusy(server.name);
        setMessage("");
        setFailures(({ [server.name]: _, ...rest }) => rest);
        try {
            setServers(await work());
            if (done) setMessage(done);
        } catch (cause) {
            const why = cause instanceof Error ? cause.message : String(cause);
            setFailures((failed) => ({ ...failed, [server.name]: why }));
        } finally {
            setBusy("");
        }
    };

    const grouped = createMemo(() => {
        const groups: Record<Group, LspStatus[]> = { installed: [], missing: [], off: [] };
        for (const server of servers()) groups[groupOf(server)].push(server);
        return groups;
    });
    const shown = () => (["installed", "missing", "off"] as const).filter((group) => grouped()[group].length);

    return (
        <div class="space-y-4">
            <Show when={message()}>
                <div role="status" class="rounded-md border border-ok/35 bg-ok/10 px-3 py-2 text-xs text-ok">
                    {message()}
                </div>
            </Show>
            <For each={shown()}>
                {(group, index) => (
                    <div aria-busy={loading()}>
                        <SettingsGroup
                            title={t(`drift.lsp.group.${group}`)}
                            action={
                                <Show when={index() === 0}>
                                    <button class={button} disabled={loading() || !!busy()} onClick={() => void load()}>
                                        {t("drift.lsp.refresh")}
                                    </button>
                                </Show>
                            }
                        >
                            <For each={grouped()[group]}>
                                {(server) => (
                                    <ServerRow
                                        server={server}
                                        busy={busy()}
                                        failure={failures()[server.name]}
                                        onInstall={() =>
                                            void act(
                                                server,
                                                () => engine.actions.installLanguageServer(server.name, here()),
                                                t("drift.lsp.installed", { name: server.label }),
                                            )
                                        }
                                        onCheck={() =>
                                            void act(server, () =>
                                                engine.actions.checkLanguageServer(server.name, here()),
                                            )
                                        }
                                        onEnabled={(enabled) =>
                                            void act(server, () =>
                                                engine.actions.setLanguageServerEnabled(server.name, enabled, here()),
                                            )
                                        }
                                    />
                                )}
                            </For>
                        </SettingsGroup>
                    </div>
                )}
            </For>
        </div>
    );
}

function ServerRow(props: {
    server: LspStatus;
    busy: string;
    failure?: string;
    onInstall: () => void;
    onCheck: () => void;
    onEnabled: (enabled: boolean) => void;
}) {
    const working = () => props.busy === props.server.name;
    const source = () => {
        if (props.server.source === "plugin")
            return t("drift.lsp.source.plugin", { plugin: props.server.plugin ?? "" });
        return props.server.source === "config" ? t("drift.lsp.source.config") : "";
    };
    const installed = () => !["missing", "off"].includes(props.server.state);
    const error = () => props.failure ?? props.server.error;

    return (
        <div
            class="settings-item-row flex items-center gap-3 border-b border-edge/70 px-3 py-2.5 last:border-b-0 hover:bg-raised/40"
            title={t("drift.lsp.benefit", { language: props.server.label })}
        >
            <div class="min-w-0 flex-1">
                <div class="flex min-w-0 items-center gap-2">
                    <span class="truncate text-sm font-medium text-ink">{props.server.label}</span>
                    <Show when={source()}>
                        <Badge>{source()}</Badge>
                    </Show>
                </div>
                <div class="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-2 text-xs">
                    <span
                        classList={{
                            "text-ok": ["running", "works"].includes(props.server.state),
                            "text-ink-muted": props.server.state === "found",
                            "text-danger": props.server.state === "failed",
                            "text-ink-faint": ["missing", "off"].includes(props.server.state),
                        }}
                    >
                        {t(`drift.lsp.state.${props.server.state}`)}
                    </span>
                    <span class="truncate font-mono text-ink-faint" title={props.server.path ?? props.server.command}>
                        {props.server.command}
                    </span>
                    <span class="truncate text-ink-faint">{props.server.extensions.join(" ")}</span>
                </div>
                <Show when={error()}>
                    {(why) => <div class="mt-1 text-xs break-words whitespace-pre-wrap text-danger">{why()}</div>}
                </Show>
            </div>
            <div class="settings-item-actions flex shrink-0 items-center gap-1.5">
                <Show when={props.server.state === "missing" && props.server.install}>
                    {(command) => (
                        <button
                            type="button"
                            class={button}
                            disabled={!!props.busy}
                            title={t("drift.lsp.installTitle", { command: command() })}
                            onClick={() => props.onInstall()}
                        >
                            {working() ? t("drift.lsp.installing") : t("drift.lsp.install")}
                        </button>
                    )}
                </Show>
                <Show when={props.server.needs?.length}>
                    <span class="text-xs text-ink-faint">
                        {t("drift.lsp.needs", { programs: (props.server.needs ?? []).join(" / ") })}
                    </span>
                </Show>
                <Show when={props.server.state === "missing" && props.server.note}>
                    {(note) => (
                        <span class="max-w-56 truncate text-xs text-ink-faint" title={note()}>
                            {note()}
                        </span>
                    )}
                </Show>
                <Show when={props.server.state === "missing" && props.server.guide}>
                    {(guide) => (
                        <button
                            type="button"
                            class={`${button} flex items-center gap-0.5`}
                            title={guide()}
                            onClick={() => openExternal(guide())}
                        >
                            {t("drift.lsp.guide")}
                            <IconArrowUpRight class="size-3" />
                        </button>
                    )}
                </Show>
                <Show when={installed()}>
                    <button
                        type="button"
                        class={button}
                        disabled={!!props.busy}
                        title={t("drift.lsp.checkTitle")}
                        onClick={() => props.onCheck()}
                    >
                        {working() ? t("drift.lsp.checking") : t("drift.lsp.check")}
                    </button>
                </Show>
                <Show when={props.server.source !== "config"}>
                    <Toggle
                        label={props.server.label}
                        checked={props.server.state !== "off"}
                        disabled={!!props.busy}
                        onChange={() => props.onEnabled(props.server.state === "off")}
                    />
                </Show>
            </div>
        </div>
    );
}
