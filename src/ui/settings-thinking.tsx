import { shownThinkingStyle, thinkingWordShown, ThinkingIndicator } from "./thinking-indicator";
import { thinkingIndicator, thinkingIndicators } from "../thinking-indicators";
import { SettingsGroup, SettingsRow } from "./settings-controls";
import { Toggle } from "./controls";
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
            width="12rem"
            onPick={props.pick}
        />
    );
}

export function ThinkingSettings() {
    const prefs = thinkingPrefs;
    const style = () => shownThinkingStyle(prefs().style, (id) => !!thinkingIndicator(id));
    // Only a figure has room to drop the word; Shimmer and Still are the word, a plugin draws its own.
    const wordFixed = () => thinkingWordShown(style(), false) || style().startsWith(pluginThinkingPrefix);
    const options = <K extends "tint" | "speed">(key: K, values: readonly ThinkingPrefs[K][]) =>
        values.map((value) => ({ id: value, label: t(`drift.settings.thinking.${key}.${value}`) }));
    const toggleLabel = () => !wordFixed() && updateThinkingPrefs({ label: !prefs().label });

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
                <Choice
                    title={t("drift.settings.thinking.tint.title")}
                    items={options("tint", thinkingTints)}
                    selected={prefs().tint}
                    pick={(tint) => updateThinkingPrefs({ tint: tint as ThinkingPrefs["tint"] })}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.speed.title")}
                description={t("drift.settings.thinking.speed.description")}
            >
                <Choice
                    title={t("drift.settings.thinking.speed.title")}
                    items={options("speed", thinkingSpeeds)}
                    selected={prefs().speed}
                    pick={(speed) => updateThinkingPrefs({ speed: speed as ThinkingPrefs["speed"] })}
                />
            </SettingsRow>
            <SettingsRow
                title={t("drift.settings.thinking.label.title")}
                description={t("drift.settings.thinking.label.description")}
                disabled={wordFixed()}
                onClick={toggleLabel}
            >
                <Toggle
                    label={t("drift.settings.thinking.label.title")}
                    checked={wordFixed() ? thinkingWordShown(style(), true) : prefs().label}
                    disabled={wordFixed()}
                    onChange={toggleLabel}
                />
            </SettingsRow>
        </SettingsGroup>
    );
}
