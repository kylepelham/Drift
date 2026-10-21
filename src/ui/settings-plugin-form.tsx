import { createSignal, For, Index, Match, Show, Switch, type JSX } from "solid-js";
import { IconEye, IconEyeOff, IconPlus, IconX } from "./icons";
import { Toggle } from "./controls";
import { t } from "../state/i18n";

import type { Draft, DraftValue, PairRow, PluginSetting, PluginSettingKind as Kind } from "../state/plugin-settings";

const field =
    "h-8 w-full min-w-0 rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none focus:border-accent";
const hint = "text-[length:calc(0.7rem*var(--settings-scale,1))] text-ink-faint";

/** A plugin's declared settings as a form: a field each, and rows with + for lists and pairs. */
export function PluginSettingsForm(props: {
    settings: PluginSetting[];
    draft: Draft;
    onDraft: (draft: Draft) => void;
}) {
    const set = (key: string, value: DraftValue) => props.onDraft({ ...props.draft, [key]: value });

    return (
        <div class="space-y-4 rounded-lg border border-edge bg-surface p-3">
            <For each={props.settings}>
                {(setting) => (
                    <SettingField
                        setting={setting}
                        value={props.draft[setting.key]}
                        onValue={(value) => set(setting.key, value)}
                    />
                )}
            </For>
        </div>
    );
}

function SettingField(props: { setting: PluginSetting; value: DraftValue; onValue: (value: DraftValue) => void }) {
    const kind = () => props.setting.kind;
    const heading = (
        <div class="min-w-0">
            <div class="text-xs font-medium text-ink">{props.setting.label}</div>
            <Show when={props.setting.description}>{(text) => <div class={hint}>{text()}</div>}</Show>
        </div>
    );

    return (
        <Switch>
            <Match when={kind().type === "toggle"}>
                <div class="flex items-center justify-between gap-3">
                    {heading}
                    <Toggle
                        label={props.setting.label}
                        checked={props.value === true}
                        onChange={() => props.onValue(props.value !== true)}
                    />
                </div>
            </Match>
            <Match when={true}>
                <div class="space-y-1.5">
                    {heading}
                    <SettingInput
                        kind={kind()}
                        label={props.setting.label}
                        value={props.value}
                        onValue={props.onValue}
                    />
                </div>
            </Match>
        </Switch>
    );
}

type KindOf<T extends Kind["type"]> = Extract<Kind, { type: T }>;

function SettingInput(props: { kind: Kind; label: string; value: DraftValue; onValue: (value: DraftValue) => void }) {
    const of = <T extends Kind["type"]>(type: T) => (props.kind.type === type ? (props.kind as KindOf<T>) : undefined);
    const text = () => String(props.value);
    const typed = (event: { currentTarget: { value: string } }) => props.onValue(event.currentTarget.value);

    return (
        <Switch>
            <Match when={of("text")?.multiline && of("text")}>
                {(kind) => (
                    <textarea
                        spellcheck={false}
                        aria-label={props.label}
                        placeholder={kind().placeholder}
                        class="h-24 w-full resize-y rounded-md border border-edge bg-raised/45 p-2.5 font-mono text-xs text-ink outline-none focus:border-accent"
                        value={text()}
                        onInput={typed}
                    />
                )}
            </Match>
            <Match when={of("text")}>
                {(kind) => (
                    <TextInput
                        label={props.label}
                        placeholder={kind().placeholder}
                        secret={kind().secret}
                        value={text()}
                        onValue={props.onValue}
                    />
                )}
            </Match>
            <Match when={of("number")}>
                {(kind) => (
                    <input
                        type="number"
                        aria-label={props.label}
                        class={field}
                        min={kind().min ?? undefined}
                        max={kind().max ?? undefined}
                        step={kind().integer ? 1 : "any"}
                        value={text()}
                        onInput={typed}
                    />
                )}
            </Match>
            <Match when={of("choice")}>
                {(kind) => (
                    <select aria-label={props.label} class={field} value={text()} onChange={typed}>
                        <For each={kind().choices}>
                            {(choice) => <option value={choice.value}>{choice.label}</option>}
                        </For>
                    </select>
                )}
            </Match>
            <Match when={of("items")}>
                {(kind) => (
                    <ItemRows
                        label={props.label}
                        placeholder={kind().placeholder}
                        secret={kind().secret}
                        rows={props.value as string[]}
                        onRows={props.onValue}
                    />
                )}
            </Match>
            <Match when={of("pairs")}>
                {(kind) => (
                    <PairRows
                        kind={kind()}
                        label={props.label}
                        rows={props.value as PairRow[]}
                        onRows={props.onValue}
                    />
                )}
            </Match>
        </Switch>
    );
}

/** One line of text; a secret one is hidden until its eye is pressed. */
function TextInput(props: {
    label: string;
    placeholder: string;
    secret: boolean;
    value: string;
    onValue: (value: string) => void;
}) {
    const [shown, setShown] = createSignal(false);
    const label = () => t(shown() ? "drift.plugins.settings.hide" : "drift.plugins.settings.show");

    return (
        <div class="flex min-w-0 flex-1 items-center gap-1">
            <input
                type={props.secret && !shown() ? "password" : "text"}
                autocomplete="off"
                spellcheck={false}
                aria-label={props.label}
                placeholder={props.placeholder}
                class={field}
                value={props.value}
                onInput={(event) => props.onValue(event.currentTarget.value)}
            />
            <Show when={props.secret}>
                <RowButton label={label()} onClick={() => setShown(!shown())}>
                    {shown() ? <IconEyeOff class="size-3.5" /> : <IconEye class="size-3.5" />}
                </RowButton>
            </Show>
        </div>
    );
}

function RowButton(props: { label: string; onClick: () => void; children: JSX.Element }) {
    return (
        <button
            type="button"
            title={props.label}
            aria-label={props.label}
            class="flex size-8 shrink-0 items-center justify-center rounded-md text-ink-faint hover:bg-raised hover:text-ink"
            onClick={() => props.onClick()}
        >
            {props.children}
        </button>
    );
}

function AddRow(props: { onAdd: () => void }) {
    return (
        <button
            type="button"
            class="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-ink-muted hover:bg-raised hover:text-ink"
            onClick={() => props.onAdd()}
        >
            <IconPlus class="size-3.5" />
            {t("drift.plugins.settings.add")}
        </button>
    );
}

function ItemRows(props: {
    label: string;
    placeholder: string;
    secret: boolean;
    rows: string[];
    onRows: (rows: string[]) => void;
}) {
    const change = (index: number, value: string) =>
        props.onRows(props.rows.map((row, at) => (at === index ? value : row)));

    return (
        <div class="space-y-1.5">
            <Index each={props.rows}>
                {(row, index) => (
                    <div class="flex items-center gap-1">
                        <TextInput
                            label={props.label}
                            placeholder={props.placeholder}
                            secret={props.secret}
                            value={row()}
                            onValue={(value) => change(index, value)}
                        />
                        <RowButton
                            label={t("drift.plugins.settings.remove")}
                            onClick={() => props.onRows(props.rows.filter((_, at) => at !== index))}
                        >
                            <IconX class="size-3.5" />
                        </RowButton>
                    </div>
                )}
            </Index>
            <AddRow onAdd={() => props.onRows([...props.rows, ""])} />
        </div>
    );
}

function PairRows(props: { kind: KindOf<"pairs">; label: string; rows: PairRow[]; onRows: (rows: PairRow[]) => void }) {
    const change = (index: number, part: Partial<PairRow>) =>
        props.onRows(props.rows.map((row, at) => (at === index ? { ...row, ...part } : row)));

    return (
        <div class="space-y-1.5">
            <Show when={props.rows.length}>
                <div class={`flex gap-1 pe-9 ${hint}`}>
                    <span class="flex-1">{props.kind.keyLabel}</span>
                    <span class="flex-1">{props.kind.valueLabel}</span>
                </div>
            </Show>
            <Index each={props.rows}>
                {(row, index) => (
                    <div class="flex items-center gap-1">
                        <TextInput
                            label={`${props.label}: ${props.kind.keyLabel}`}
                            placeholder={props.kind.keyLabel}
                            secret={props.kind.secretKeys}
                            value={row().key}
                            onValue={(key) => change(index, { key })}
                        />
                        <TextInput
                            label={`${props.label}: ${props.kind.valueLabel}`}
                            placeholder={props.kind.valueLabel}
                            secret={props.kind.secretValues}
                            value={row().value}
                            onValue={(value) => change(index, { value })}
                        />
                        <RowButton
                            label={t("drift.plugins.settings.remove")}
                            onClick={() => props.onRows(props.rows.filter((_, at) => at !== index))}
                        >
                            <IconX class="size-3.5" />
                        </RowButton>
                    </div>
                )}
            </Index>
            <AddRow onAdd={() => props.onRows([...props.rows, { key: "", value: "" }])} />
        </div>
    );
}
