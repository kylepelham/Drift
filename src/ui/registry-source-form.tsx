import { sourceProblem } from "../state/registry-sources";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import { Show } from "solid-js";

import type { RegistryKind, SourceKind } from "../state/registry-sources";
import type { Draft } from "./registry-sources";
import type { JSX } from "solid-js";

const sourceKinds: SourceKind[] = ["url", "github", "azure_devops", "folder"];
const fieldInputClass =
    "h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent";

export function SourceForm(props: {
    kind: RegistryKind;
    draft: Draft;
    busy: boolean;
    onChange: (draft: Draft) => void;
    onCancel: () => void;
    onSave: () => void;
}) {
    const updateDraft = (change: Partial<Draft>) => props.onChange({ ...props.draft, ...change });
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
                        class={fieldInputClass}
                        value={props.draft.name}
                        onInput={(event) => updateDraft({ name: event.currentTarget.value })}
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
                        onPick={(kind) => updateDraft({ source: kind as SourceKind })}
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
                        class={`${fieldInputClass} font-mono`}
                        spellcheck={false}
                        placeholder={placeholder()}
                        value={props.draft.url}
                        onInput={(event) => updateDraft({ url: event.currentTarget.value })}
                    />
                </Field>
                <Show when={isRepo()}>
                    <div class="grid gap-3 sm:grid-cols-2">
                        <Field label={t("drift.registry.sources.ref")} hint={t("drift.registry.sources.refHint")}>
                            <input
                                class={`${fieldInputClass} font-mono`}
                                spellcheck={false}
                                placeholder="main"
                                value={props.draft.ref}
                                onInput={(event) => updateDraft({ ref: event.currentTarget.value })}
                            />
                        </Field>
                        <Field label={t("drift.registry.sources.path")} hint={t("drift.registry.sources.pathHint")}>
                            <input
                                class={`${fieldInputClass} font-mono`}
                                spellcheck={false}
                                placeholder="registry.json"
                                value={props.draft.path}
                                onInput={(event) => updateDraft({ path: event.currentTarget.value })}
                            />
                        </Field>
                    </div>
                </Show>
                <Show when={props.draft.source === "folder"}>
                    <Field label={t("drift.registry.sources.path")} hint={t("drift.registry.sources.pathHint")}>
                        <input
                            class={`${fieldInputClass} font-mono`}
                            spellcheck={false}
                            placeholder="registry.json"
                            value={props.draft.path}
                            onInput={(event) => updateDraft({ path: event.currentTarget.value })}
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
                            class={`${fieldInputClass} font-mono`}
                            placeholder={props.draft.hasToken && !props.draft.tokenTouched ? "••••••••" : ""}
                            value={props.draft.token}
                            onInput={(event) => updateDraft({ token: event.currentTarget.value, tokenTouched: true })}
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
                            onChange={() => updateDraft({ allowHttp: !props.draft.allowHttp })}
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
                            onInput={(event) => updateDraft({ caPem: event.currentTarget.value })}
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
