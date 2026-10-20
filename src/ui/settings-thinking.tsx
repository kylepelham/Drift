import { thinkingIndicator, thinkingIndicators } from "../thinking-indicators";
import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { shownThinkingStyle, ThinkingIndicator } from "./thinking-indicator";
import { Segmented, Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    pluginThinkingPrefix,
    thinkingPrefs,
    thinkingSpeeds,
    thinkingStyles,
    thinkingTints,
    updateThinkingPrefs,
} from "../state/prefs";

import type { ThinkingPrefs } from "../state/prefs";

/** The built-in styles, then each one a plugin registered. */
export function thinkingStyleItems(plugins: { id: string; name: string }[]) {
    return [
        ...thinkingStyles.map((style) => ({ id: style, label: t(`drift.settings.thinking.style.${style}`) })),
        ...plugins.map((plugin) => ({
            id: `${pluginThinkingPrefix}${plugin.id}`,
            label: t("drift.settings.thinking.style.plugin", { name: plugin.name }),
        })),
    ];
}

function Choice(props: {
    title: string;
    items: { id: string; label: string }[];
    selected: string;
    pick: (id: string) => void;
}) {
    return (
        <Picker
            label={props.title}
            items={props.items}
            selected={props.selected}
            floating
            bordered
            chevronAtEnd
            placement="below"
            width={pickerWidth}
            onPick={props.pick}
        />
    );
}

export function ThinkingSettings() {
    const prefs = thinkingPrefs;
    const style = () => shownThinkingStyle(prefs().style, (id) => !!thinkingIndicator(id));
    const options = <K extends "tint" | "speed">(key: K, values: readonly ThinkingPrefs[K][]) =>
        values.map((value) => ({ id: value, label: t(`drift.settings.thinking.${key}.${value}`) }));
    const toggleLabel = () => updateThinkingPrefs({ label: !prefs().label });
    const toggleShimmer = () => prefs().label && updateThinkingPrefs({ shimmer: !prefs().shimmer });

    return (
        <SettingsGroup title={t("drift.settings.thinking")}>
            <SettingsRow
                title={t("drift.settings.thinking.preview.title")}
                description={t("drift.settings.thinking.preview.description")}
            >
                <ThinkingIndicator preview />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.style.title")}
                description={t("drift.settings.thinking.style.description")}
            >
                <Choice
                    title={t("drift.settings.thinking.style.title")}
                    items={thinkingStyleItems(thinkingIndicators())}
                    selected={style()}
                    pick={(id) => updateThinkingPrefs({ style: id })}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.tint.title")}
                description={t("drift.settings.thinking.tint.description")}
            >
                <Segmented
                    label={t("drift.settings.thinking.tint.title")}
                    options={options("tint", thinkingTints)}
                    value={prefs().tint}
                    onChange={(tint) => updateThinkingPrefs({ tint })}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.speed.title")}
                description={t("drift.settings.thinking.speed.description")}
            >
                <Segmented
                    label={t("drift.settings.thinking.speed.title")}
                    options={options("speed", thinkingSpeeds)}
                    value={prefs().speed}
                    onChange={(speed) => updateThinkingPrefs({ speed })}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.label.title")}
                description={t("drift.settings.thinking.label.description")}
                onClick={toggleLabel}
            >
                <div class="flex items-center gap-2.5">
                    <input
                        type="text"
                        maxLength={40}
                        aria-label={t("drift.settings.thinking.text.label")}
                        placeholder={t("drift.chat.thinking")}
                        class="h-8 rounded-md border border-edge-strong bg-bg/70 px-2.5 text-xs text-ink outline-none placeholder:text-ink-faint focus:border-accent"
                        style={{ width: pickerWidth }}
                        value={prefs().text}
                        onClick={(event) => event.stopPropagation()}
                        onInput={(event) => updateThinkingPrefs({ text: event.currentTarget.value })}
                    />
                    <Toggle
                        label={t("drift.settings.thinking.label.title")}
                        checked={prefs().label}
                        onChange={toggleLabel}
                    />
                </div>
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.shimmer.title")}
                description={t("drift.settings.thinking.shimmer.description")}
                disabled={!prefs().label}
                onClick={toggleShimmer}
            >
                <Toggle
                    label={t("drift.settings.thinking.shimmer.title")}
                    checked={prefs().shimmer}
                    disabled={!prefs().label}
                    onChange={toggleShimmer}
                />
            </SettingsRow>
        </SettingsGroup>
    );
}
