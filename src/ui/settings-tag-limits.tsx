import { createSignal, Index, onMount, Show } from "solid-js";
import { SettingsRow } from "./settings-controls";
import { IconPlus, IconX } from "./icons";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

type Row = { tag: string; limit: string };

const field =
    "h-8 min-w-0 rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none focus:border-accent";

const complete = (row: Row) => row.tag.trim() !== "" && row.limit.trim() !== "";

/** How many subagents with each tag run at once; the engine owns the values, and a row is saved once it is left. */
export function TagLimitsRow() {
    const engine = useEngine();
    const [rows, setRows] = createSignal<Row[]>([]);
    const [error, setError] = createSignal("");
    let saves = 0;
    const shown = (limits: Record<string, number> | null | undefined) =>
        Object.entries(limits ?? {}).map(([tag, limit]) => ({ tag, limit: String(limit) }));

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setRows(shown(settings.taskTagLimits)))
                .catch(() => undefined),
    );

    async function save(next: Row[]) {
        const save = ++saves;
        setRows(next);
        setError("");
        const limits = Object.fromEntries(next.filter(complete).map((row) => [row.tag.trim(), Number(row.limit)]));

        try {
            const settings = await engine.actions.setTaskTagLimits(limits);
            // Only the latest save's answer is shown; an earlier one would drop rows filled in since.
            if (save !== saves) return;
            // The engine's own form of each saved tag, with rows still being filled in kept below them.
            setRows([...shown(settings.taskTagLimits), ...rows().filter((row) => !complete(row))]);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        }
    }

    const edited = (index: number, part: Partial<Row>) =>
        rows().map((row, at) => (at === index ? { ...row, ...part } : row));

    return (
        <>
            <SettingsRow
                title={t("drift.settings.tagLimits.title")}
                description={t("drift.settings.tagLimits.description")}
            >
                <div class="flex flex-col items-end gap-1.5">
                    <Index each={rows()}>
                        {(row, index) => (
                            <div class="flex items-center gap-1">
                                <input
                                    type="text"
                                    autocomplete="off"
                                    spellcheck={false}
                                    aria-label={t("drift.settings.tagLimits.tag")}
                                    placeholder={t("drift.settings.tagLimits.tag")}
                                    class={`${field} w-28`}
                                    value={row().tag}
                                    onInput={(event) => setRows(edited(index, { tag: event.currentTarget.value }))}
                                    onChange={(event) => void save(edited(index, { tag: event.currentTarget.value }))}
                                />
                                <input
                                    type="number"
                                    min="1"
                                    max="64"
                                    step="1"
                                    aria-label={t("drift.settings.tagLimits.limit")}
                                    placeholder={t("drift.settings.tagLimits.limit")}
                                    class={`${field} w-20`}
                                    value={row().limit}
                                    onInput={(event) => setRows(edited(index, { limit: event.currentTarget.value }))}
                                    onChange={(event) => void save(edited(index, { limit: event.currentTarget.value }))}
                                />
                                <button
                                    type="button"
                                    title={t("drift.plugins.settings.remove")}
                                    aria-label={t("drift.plugins.settings.remove")}
                                    class="flex size-8 shrink-0 items-center justify-center rounded-md text-ink-faint hover:bg-raised hover:text-ink"
                                    onClick={() => void save(rows().filter((_, at) => at !== index))}
                                >
                                    <IconX class="size-3.5" />
                                </button>
                            </div>
                        )}
                    </Index>
                    <button
                        type="button"
                        class="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-ink-muted hover:bg-raised hover:text-ink"
                        onClick={() => setRows([...rows(), { tag: "", limit: "" }])}
                    >
                        <IconPlus class="size-3.5" />
                        {t("drift.plugins.settings.add")}
                    </button>
                </div>
            </SettingsRow>
            <Show when={error()}>
                <div class="text-xs text-danger">{error()}</div>
            </Show>
        </>
    );
}
