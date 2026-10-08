import { IconArrowUp, IconKey, IconPlus, IconSquarePen, IconTrash } from "./icons";
import { createSignal, For, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    loadRegistrySources,
    registrySources,
    saveRegistrySources,
    sourceProblem,
    type RegistryKind,
    type RegistrySource,
    type SourceKind,
} from "../state/registry-sources";

import type { JSX } from "solid-js";

const sourceKinds: SourceKind[] = ["url", "github", "azure_devops", "folder"];

type Draft = {
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

const blank = (): Draft => ({
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

/** Registries of one kind the user added, each editable; shared by the plugin, skill and MCP pages. */
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
    const mine = () => registrySources().filter((source) => source.kind === props.kind);

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
                            <For each={mine()}>
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
                            <Show when={!mine().length}>
                                <div class="px-3 py-4 text-sm text-ink-faint">{t("drift.registry.sources.empty")}</div>
                            </Show>
                        </div>
                        <button
                            class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted hover:border-edge-strong hover:text-ink disabled:opacity-40"
                            disabled={busy()}
                            onClick={() => setDraft(blank())}
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

function SourceForm(props: {
    kind: RegistryKind;
    draft: Draft;
    busy: boolean;
    onChange: (draft: Draft) => void;
    onCancel: () => void;
    onSave: () => void;
}) {
    const set = (change: Partial<Draft>) => props.onChange({ ...props.draft, ...change });
    const problem = () => sourceProblem(props.draft.source, props.draft.url, props.draft.allowHttp);
    const canSave = () => props.draft.name.trim().length > 0 && problem() === undefined && !props.busy;
    const isRepo = () => props.draft.source === "github" || props.draft.source === "azure_devops";
    const isHttp = () => props.draft.source !== "folder";
    const placeholder = () =>
        ({
            url: "https://registry.example.com/plugins.json",
            github: "https://github.com/acme/drift-plugins",
            azure_devops: "https://dev.azure.com/acme/Tools/_git/drift-plugins",
            folder: "\\\\fileserver\\drift\\registry",
        })[props.draft.source];
    return (
        <div class="space-y-4">
            <div class="text-base font-semibold text-ink">
                {t(props.draft.id ? "drift.registry.sources.editTitle" : "drift.registry.sources.add")}
            </div>
            <div class="space-y-3 rounded-lg border border-edge bg-surface p-3">
                <Field label={t("drift.registry.sources.name")}>
                    <input
                        class={input}
                        value={props.draft.name}
                        onInput={(event) => set({ name: event.currentTarget.value })}
                    />
                </Field>
                <Field
                    label={t("drift.registry.sources.kindLabel")}
                    hint={t(`drift.registry.sources.kindHint.${props.draft.source}`)}
                >
                    <Picker
                        label={t("drift.registry.sources.kindLabel")}
                        items={sourceKinds.map((kind) => ({
                            id: kind,
                            label: t(`drift.registry.sources.kind.${kind}`),
                        }))}
                        selected={props.draft.source}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="13rem"
                        onPick={(kind) => set({ source: kind as SourceKind })}
                    />
                </Field>
                <Field
                    label={t(`drift.registry.sources.location.${props.draft.source}`)}
                    problem={
                        problem() && problem() !== "empty"
                            ? t(`drift.registry.sources.problem.${problem()}`)
                            : undefined
                    }
                >
                    <input
                        class={`${input} font-mono`}
                        spellcheck={false}
                        placeholder={placeholder()}
                        value={props.draft.url}
                        onInput={(event) => set({ url: event.currentTarget.value })}
                    />
                </Field>
                <Show when={isRepo()}>
                    <div class="grid gap-3 sm:grid-cols-2">
                        <Field label={t("drift.registry.sources.ref")} hint={t("drift.registry.sources.refHint")}>
                            <input
                                class={`${input} font-mono`}
                                spellcheck={false}
                                placeholder="main"
                                value={props.draft.ref}
                                onInput={(event) => set({ ref: event.currentTarget.value })}
                            />
                        </Field>
                        <Field label={t("drift.registry.sources.path")} hint={t("drift.registry.sources.pathHint")}>
                            <input
                                class={`${input} font-mono`}
                                spellcheck={false}
                                placeholder="registry.json"
                                value={props.draft.path}
                                onInput={(event) => set({ path: event.currentTarget.value })}
                            />
                        </Field>
                    </div>
                </Show>
                <Show when={props.draft.source === "folder"}>
                    <Field label={t("drift.registry.sources.path")} hint={t("drift.registry.sources.pathHint")}>
                        <input
                            class={`${input} font-mono`}
                            spellcheck={false}
                            placeholder="registry.json"
                            value={props.draft.path}
                            onInput={(event) => set({ path: event.currentTarget.value })}
                        />
                    </Field>
                </Show>
                <Show when={isHttp()}>
                    <Field
                        label={t(`drift.registry.sources.token.${props.draft.source}`)}
                        hint={
                            props.draft.hasToken && !props.draft.tokenTouched
                                ? t("drift.registry.sources.tokenKept")
                                : t("drift.registry.sources.tokenHint")
                        }
                    >
                        <input
                            type="password"
                            autocomplete="off"
                            class={`${input} font-mono`}
                            placeholder={props.draft.hasToken && !props.draft.tokenTouched ? "••••••••" : ""}
                            value={props.draft.token}
                            onInput={(event) => set({ token: event.currentTarget.value, tokenTouched: true })}
                        />
                    </Field>
                </Show>
                <Show when={props.draft.source === "url"}>
                    <div class="flex items-center justify-between gap-3">
                        <div class="min-w-0">
                            <div class="text-xs font-medium text-ink">{t("drift.registry.sources.allowHttp")}</div>
                            <div class="text-[0.7rem] text-warn">{t("drift.registry.sources.allowHttpHint")}</div>
                        </div>
                        <Toggle
                            label={t("drift.registry.sources.allowHttp")}
                            checked={props.draft.allowHttp}
                            onChange={() => set({ allowHttp: !props.draft.allowHttp })}
                        />
                    </div>
                </Show>
                <Show when={isHttp()}>
                    <Field label={t("drift.registry.sources.caPem")} hint={t("drift.registry.sources.caPemHint")}>
                        <textarea
                            spellcheck={false}
                            class="h-24 w-full resize-y rounded-md border border-edge bg-raised/45 p-2.5 font-mono text-[0.7rem] text-ink outline-none focus:border-accent"
                            placeholder={"-----BEGIN CERTIFICATE-----"}
                            value={props.draft.caPem}
                            onInput={(event) => set({ caPem: event.currentTarget.value })}
                        />
                    </Field>
                </Show>
            </div>
            <div class="flex items-center justify-end gap-2">
                <button
                    class="rounded-md px-3 py-1.5 text-xs text-ink-muted hover:text-ink"
                    onClick={() => props.onCancel()}
                >
                    {t("common.cancel")}
                </button>
                <button
                    class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
                    disabled={!canSave()}
                    onClick={() => props.onSave()}
                >
                    {t(props.busy ? "drift.plugins.saving" : "common.save")}
                </button>
            </div>
        </div>
    );
}

const input =
    "h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent";

function Field(props: { label: string; hint?: string; problem?: string; children: JSX.Element }) {
    return (
        <label class="block space-y-1">
            <div class="text-xs font-medium text-ink">{props.label}</div>
            {props.children}
            <Show
                when={props.problem}
                fallback={
                    <Show when={props.hint}>{(hint) => <div class="text-[0.7rem] text-ink-faint">{hint()}</div>}</Show>
                }
            >
                {(problem) => <div class="text-[0.7rem] text-danger">{problem()}</div>}
            </Show>
        </label>
    );
}
