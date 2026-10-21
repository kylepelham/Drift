import { firstFamily, fontList, systemFonts } from "../state/fonts";
import { pickerWidth } from "./settings-controls";
import { createMemo, Show } from "solid-js";
import { t } from "../state/i18n";
import { Picker } from "./picker";

/** What each font setting falls back to when it is left at its default. */
export const defaultFonts = {
    ui: "Segoe UI Variable Text",
    code: "Cascadia Code",
};

/** A free-text font list, for where the installed fonts cannot be listed, with a reset to the default. */
function FontField(props: {
    label: string;
    value: string;
    onInput: (value: string) => void;
    mono?: boolean;
    fallback: string;
}) {
    return (
        <div class="flex items-center gap-2">
            <input
                aria-label={props.label}
                class="h-8 rounded-md border border-edge-strong bg-bg/70 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent"
                style={{ width: pickerWidth }}
                classList={{ "font-mono text-xs": props.mono }}
                placeholder={props.fallback}
                maxLength={256}
                value={props.value}
                onInput={(event) => props.onInput(event.currentTarget.value)}
            />
            <Show when={props.value.trim()}>
                <button
                    class="h-8 shrink-0 rounded-md border border-edge px-2.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                    onClick={() => props.onInput("")}
                >
                    {t("drift.font.reset")}
                </button>
            </Show>
        </div>
    );
}

/**
 * The installed fonts, each shown in its own face, after the default the app comes with; code fonts list only
 * monospaced ones. Falls back to typing a name where the fonts cannot be listed.
 */
export function FontPicker(props: {
    label: string;
    value: string;
    onInput: (value: string) => void;
    mono?: boolean;
    /** The family used when the setting is left empty. */
    fallback: string;
}) {
    const listed = createMemo(() => (systemFonts() ?? []).filter((font) => !props.mono || font.monospace));
    const defaultLabel = () => t("drift.font.defaultNamed", { font: props.fallback });
    const items = createMemo(() => [
        { id: "", label: defaultLabel(), font: fontList(props.fallback, !!props.mono) },
        ...listed().map((font) => ({ id: font.family, label: font.family, font: fontList(font.family, !!props.mono) })),
    ]);
    const current = () => firstFamily(props.value);

    return (
        <Show
            when={listed().length > 0}
            fallback={
                <FontField
                    label={props.label}
                    value={props.value}
                    onInput={props.onInput}
                    mono={props.mono}
                    fallback={props.fallback}
                />
            }
        >
            <Picker
                label={props.label}
                items={items()}
                selected={current()}
                fallbackLabel={current() || defaultLabel()}
                floating
                bordered
                chevronAtEnd
                placement="below"
                width={pickerWidth}
                onPick={(family) => props.onInput(family ? fontList(family, !!props.mono) : "")}
            />
        </Show>
    );
}
