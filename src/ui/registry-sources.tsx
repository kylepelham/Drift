import { IconArrowUp, IconKey, IconPlus, IconSquarePen, IconTrash } from "./icons";
import { createSignal, For, onMount, Show } from "solid-js";
import { SourceForm } from "./registry-source-form";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    loadRegistrySources,
    registrySources,
    saveRegistrySources,
    type RegistryKind,
    type RegistrySource,
    type SourceKind,
} from "../state/registry-sources";

export type Draft = {
    id: string;
    name: string;
    source: SourceKind;
    url: string;
    ref: string;
    path: string;
    token: string;
    tokenTouched: boolean;
    allowHttp: boolean;
    caPem: string;
    hasToken: boolean;
};

const blankDraft = (): Draft => ({
    id: "",
    name: "",
    source: "url",
    url: "",
    ref: "",
    path: "",
    token: "",
    tokenTouched: false,
    allowHttp: false,
    caPem: "",
    hasToken: false,
});
const fromSource = (source: RegistrySource): Draft => ({
    id: source.id,
    name: source.name,
    source: source.source ?? "url",
    url: source.url,
    ref: source.ref ?? "",
    path: source.path ?? "",
    token: "",
    tokenTouched: false,
    allowHttp: !!source.allowHttp,
    caPem: source.caPem ?? "",
    hasToken: !!source.hasToken,
});

/** Lists and edits user registries for the plugin, skill, or MCP page. */
export function RegistrySourcesSheet(props: { kind: RegistryKind; onBack: () => void }) {
    const engine = useEngine();
    const [draft, setDraft] = createSignal<Draft>();
    const [busy, setBusy] = createSignal(false);
    const [error, setError] = createSignal("");
    const client = () => ({
        settings: () => engine.actions.engineSettings(),
        putSettings: (body: Parameters<typeof engine.actions.putEngineSettings>[0]) =>
            engine.actions.putEngineSettings(body),
    });
    onMount(() => void loadRegistrySources(client()).catch(() => setError(t("drift.registry.sources.loadFailed"))));

    const ownSources = () => registrySources().filter((source) => source.kind === props.kind);

    const save = async (next: RegistrySource[], tokens: Record<string, string>) => {
        setBusy(true);
        setError("");

        try {
            await saveRegistrySources(client(), next, tokens);
            return true;
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
            return false;
        } finally {
            setBusy(false);
        }
    };

    const submit = async (edited: Draft) => {
        const id = edited.id || crypto.randomUUID().replaceAll("-", "").slice(0, 16);
        const source: RegistrySource = {
            id,
            name: edited.name.trim(),
            kind: props.kind,
            source: edited.source,
            url: edited.url.trim(),
            ref: edited.ref.trim(),
            path: edited.path.trim(),
            allowHttp: edited.allowHttp,
            caPem: edited.caPem.trim() || null,
            hasToken: edited.hasToken,
        };

        const rest = registrySources().filter((item) => item.id !== id);
        const tokens = edited.tokenTouched ? { [id]: edited.token.trim() } : {};

        if (await save([...rest, source], tokens)) setDraft(undefined);
    };
    const remove = (source: RegistrySource) =>
        void save(
            registrySources().filter((item) => item.id !== source.id),
            {},
        );

    return (
        <div class="space-y-4">
            <button
                class="flex items-center gap-1.5 text-xs text-ink-faint hover:text-ink"
                onClick={() => (draft() ? setDraft(undefined) : props.onBack())}
            >
                <IconArrowUp class="size-3.5 -rotate-90" />
                {t("drift.mcp.registry.back")}
            </button>
            <Show when={error()}>
                <div role="alert" class="rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-xs text-danger">
                    {error()}
                </div>
            </Show>
            <Show
                when={draft()}
                fallback={
                    <>
                        <div>
                            <div class="text-base font-semibold text-ink">{t("drift.registry.sources")}</div>
                            <div class="mt-1 text-sm text-ink-muted">
                                {t(
                                    props.kind === "plugins"
                                        ? "drift.registry.sources.pluginsDescription"
                                        : "drift.registry.sources.mcpDescription",
                                )}
                            </div>
                        </div>
                        <div class="border-y border-edge/80">
                            <For each={ownSources()}>
                                {(source) => (
                                    <div class="flex items-center gap-3 border-b border-edge/70 px-3 py-2.5 last:border-b-0">
                                        <div class="min-w-0 flex-1">
                                            <div class="flex items-center gap-2">
                                                <span class="truncate text-sm font-medium text-ink">{source.name}</span>
                                                <span class="shrink-0 rounded bg-raised px-1.5 py-0.5 text-[0.65rem] text-ink-muted">
                                                    {t(`drift.registry.sources.kind.${source.source ?? "url"}`)}
                                                </span>
                                                <Show when={source.hasToken}>
                                                    <IconKey
                                                        class="size-3 shrink-0 text-ink-faint"
                                                        aria-label={t("drift.registry.sources.hasToken")}
                                                    />
                                                </Show>
                                            </div>
                                            <div class="truncate font-mono text-xs text-ink-faint">
                                                {source.url}
                                                {source.ref ? ` @ ${source.ref}` : ""}
                                            </div>
                                        </div>
                                        <button
                                            type="button"
                                            title={t("common.edit")}
                                            aria-label={t("common.edit")}
                                            class="flex size-7 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted hover:text-ink disabled:opacity-40"
                                            disabled={busy()}
                                            onClick={() => setDraft(fromSource(source))}
                                        >
                                            <IconSquarePen class="size-3.5" />
                                        </button>
                                        <button
                                            type="button"
                                            title={t("drift.registry.sources.remove")}
                                            aria-label={t("drift.registry.sources.remove")}
                                            class="flex size-7 shrink-0 items-center justify-center rounded-md border border-danger/40 text-danger hover:bg-danger/10 disabled:opacity-40"
                                            disabled={busy()}
                                            onClick={() => remove(source)}
                                        >
                                            <IconTrash class="size-3.5" />
                                        </button>
                                    </div>
                                )}
                            </For>
                            <Show when={!ownSources().length}>
                                <div class="px-3 py-4 text-sm text-ink-faint">{t("drift.registry.sources.empty")}</div>
                            </Show>
                        </div>
                        <button
                            class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted hover:border-edge-strong hover:text-ink disabled:opacity-40"
                            disabled={busy()}
                            onClick={() => setDraft(blankDraft())}
                        >
                            <IconPlus class="size-3.5" />
                            {t("drift.registry.sources.add")}
                        </button>
                    </>
                }
            >
                {(current) => (
                    <SourceForm
                        kind={props.kind}
                        draft={current()}
                        busy={busy()}
                        onChange={setDraft}
                        onCancel={() => setDraft(undefined)}
                        onSave={() => void submit(current())}
                    />
                )}
            </Show>
        </div>
    );
}
