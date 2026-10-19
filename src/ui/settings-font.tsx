import { firstFamily, fontList, systemFonts } from "../state/fonts";
import { createMemo, Show } from "solid-js";
import { t } from "../state/i18n";
import { Picker } from "./picker";

/** A free-text font list, for where the installed fonts cannot be listed. */
function FontField(props: { label: string; value: string; onInput: (value: string) => void; mono?: boolean }) {
    return (
        <input
            aria-label={props.label}
            class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-accent sm:w-56"
            classList={{ "font-mono text-xs": props.mono }}
            placeholder={props.mono ? '"Cascadia Code", monospace' : '"Segoe UI", sans-serif'}
            maxLength={256}
            value={props.value}
            onInput={(event) => props.onInput(event.currentTarget.value)}
        />
    );
}

/**
 * The installed fonts, each shown in its own face, with the default first; code fonts list only monospaced ones.
 * Falls back to typing a name where the fonts cannot be listed.
 */
export function FontPicker(props: { label: string; value: string; onInput: (value: string) => void; mono?: boolean }) {
    const listed = createMemo(() => (systemFonts() ?? []).filter((font) => !props.mono || font.monospace));
    const items = createMemo(() => [
        { id: "", label: t("drift.font.default") },
        ...listed().map((font) => ({ id: font.family, label: font.family, font: fontList(font.family, !!props.mono) })),
    ]);
    const current = () => firstFamily(props.value);

    return (
        <Show
            when={listed().length > 0}
            fallback={<FontField label={props.label} value={props.value} onInput={props.onInput} mono={props.mono} />}
        >
            <Picker
                label={props.label}
                items={items()}
                selected={current()}
                fallbackLabel={current() || t("drift.font.default")}
                floating
                bordered
                chevronAtEnd
                placement="below"
                width="14rem"
                onPick={(family) => props.onInput(family ? fontList(family, !!props.mono) : "")}
            />
        </Show>
    );
}
