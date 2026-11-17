import { panelForm, panelLine, panelRemove, SettingsDisclosure } from "./settings-controls";
import { createSignal, For, onMount, Show } from "solid-js";
import { useEngine } from "../engine";
import { IconTrash } from "./icons";
import { t } from "../state/i18n";

type Limits = Record<string, number>;

const field =
    "h-7 min-w-0 rounded-md border border-edge bg-surface px-2.5 font-mono text-xs text-ink outline-none focus:border-edge-strong";
const button = "rounded-md border border-edge px-2.5 py-1 text-xs text-ink-muted hover:text-ink disabled:opacity-40";

/** The limits as one line, for the row while it is closed. */
function limitsSummary(limits: Limits) {
    const entries = Object.entries(limits);
    if (entries.length === 0) return t("drift.settings.tagLimits.none");

    return entries.map(([tag, limit]) => t("drift.settings.tagLimits.entry", { tag, limit })).join(" \u00b7 ");
}

/** How many subagents with each tag run at once, listed in a panel the row opens; the engine owns the values. */
export function TagLimitsRow() {
    const engine = useEngine();
    const [limits, setLimits] = createSignal<Limits>({});
    const [tag, setTag] = createSignal("");
    const [limit, setLimit] = createSignal("");
    const [error, setError] = createSignal("");

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setLimits(settings.taskTagLimits ?? {}))
                .catch(() => undefined),
    );

    async function save(next: Limits) {
        setError("");
        try {
            setLimits((await engine.actions.setTaskTagLimits(next)).taskTagLimits ?? {});
            return true;
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
            return false;
        }
    }

    const without = (name: string) => Object.fromEntries(Object.entries(limits()).filter(([other]) => other !== name));
    const ready = () => tag().trim() !== "" && Number(limit()) >= 1;
    const add = async () => {
        if (!ready()) return;
        if (await save({ ...limits(), [tag().trim()]: Number(limit()) })) {
            setTag("");
            setLimit("");
        }
    };

    return (
        <SettingsDisclosure
            title={t("drift.settings.tagLimits.title")}
            description={limitsSummary(limits())}
            label={t("drift.settings.tagLimits.manage")}
        >
            <For each={Object.entries(limits())}>
                {([name, value]) => (
                    <div class={panelLine}>
                        <span class="min-w-0 flex-1 truncate font-mono text-xs text-ink-muted">{name}</span>
                        <input
                            type="number"
                            min="1"
                            max="64"
                            step="1"
                            aria-label={t("drift.settings.tagLimits.limitFor", { tag: name })}
                            class={`${field} w-16`}
                            value={value}
                            onChange={(event) => void save({ ...limits(), [name]: Number(event.currentTarget.value) })}
                        />
                        <span class="text-xs text-ink-faint">{t("drift.settings.tagLimits.atOnce")}</span>
                        <button
                            class={panelRemove}
                            aria-label={t("drift.settings.tagLimits.remove", { tag: name })}
                            title={t("drift.settings.tagLimits.remove", { tag: name })}
                            onClick={() => void save(without(name))}
                        >
                            <IconTrash class="size-3.5" />
                        </button>
                    </div>
                )}
            </For>
            <div class={panelForm}>
                <input
                    type="text"
                    autocomplete="off"
                    spellcheck={false}
                    aria-label={t("drift.settings.tagLimits.tag")}
                    placeholder={t("drift.settings.tagLimits.tag")}
                    class={`${field} flex-1`}
                    value={tag()}
                    onInput={(event) => setTag(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void add()}
                />
                <input
                    type="number"
                    min="1"
                    max="64"
                    step="1"
                    aria-label={t("drift.settings.tagLimits.limit")}
                    placeholder={t("drift.settings.tagLimits.limit")}
                    class={`${field} w-24`}
                    value={limit()}
                    onInput={(event) => setLimit(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === "Enter" && void add()}
                />
                <button class={button} disabled={!ready()} onClick={() => void add()}>
                    {t("drift.settings.tagLimits.add")}
                </button>
            </div>
            <p class="px-3 pb-2.5 text-xs leading-relaxed text-ink-faint">
                {t("drift.settings.tagLimits.description")}
            </p>
            <Show when={error()}>
                <p class="px-3 pb-2.5 text-xs text-danger">{error()}</p>
            </Show>
        </SettingsDisclosure>
    );
}
