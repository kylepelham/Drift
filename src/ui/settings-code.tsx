import { SettingsGroup, SettingsRow } from "./settings-controls";
import { codeFont, setCodeFont } from "../state/theme";
import { FontField } from "./settings-font";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    codeFontSize,
    codeFontSizes,
    codeTabWidth,
    codeTabWidths,
    codeWordWrap,
    diffIndicator,
    diffIndicators,
    diffLineNumbers,
    diffWordWrap,
    setCodeFontSize,
    setCodeTabWidth,
    setCodeWordWrap,
    setDiffIndicator,
    setDiffLineNumbers,
    setDiffWordWrap,
    setSyntaxThemePreset,
    syntaxThemePreset,
    syntaxThemePresets,
} from "../state/code";

import type { DiffIndicator, SyntaxThemePreset } from "../state/code";

const syntaxThemeLabels: Record<SyntaxThemePreset, string> = {
    automatic: "drift.code.theme.automatic",
    github: "drift.code.theme.github",
    vitesse: "drift.code.theme.vitesse",
    one: "drift.code.theme.one",
    dracula: "drift.code.theme.dracula",
    nord: "drift.code.theme.nord",
};
const diffIndicatorLabels: Record<DiffIndicator, string> = {
    symbols: "drift.code.diffIndicators.symbols",
    bars: "drift.code.diffIndicators.bars",
    background: "drift.code.diffIndicators.background",
};

export function codeSettingOptions() {
    return {
        themes: [...syntaxThemePresets],
        fontSizes: [...codeFontSizes],
        tabWidths: [...codeTabWidths],
        indicators: [...diffIndicators],
    };
}

export function CodeSection() {
    return (
        <div class="space-y-6">
            <SettingsGroup title={t("drift.code.syntax")}>
                <SettingsRow
                    title={t("drift.code.syntaxTheme.title")}
                    description={t("drift.code.syntaxTheme.description")}
                >
                    <Picker
                        label={t("drift.code.syntaxTheme.title")}
                        items={syntaxThemePresets.map((name) => ({ id: name, label: t(syntaxThemeLabels[name]) }))}
                        selected={syntaxThemePreset()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="12rem"
                        onPick={(value) => setSyntaxThemePreset(value as SyntaxThemePreset)}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("settings.general.row.font.title")}
                    description={t("settings.general.row.font.description")}
                >
                    <FontField
                        label={t("settings.general.row.font.title")}
                        value={codeFont()}
                        onInput={setCodeFont}
                        mono
                    />
                </SettingsRow>
            </SettingsGroup>
            <SettingsGroup title={t("drift.code.layout")}>
                <SettingsRow title={t("drift.code.fontSize.title")} description={t("drift.code.fontSize.description")}>
                    <Picker
                        label={t("drift.code.fontSize.title")}
                        items={codeFontSizes.map((size) => ({ id: String(size), label: `${size} px` }))}
                        selected={String(codeFontSize())}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="8rem"
                        onPick={(value) => setCodeFontSize(Number(value))}
                    />
                </SettingsRow>
                <SettingsRow title={t("drift.code.tabWidth.title")} description={t("drift.code.tabWidth.description")}>
                    <Picker
                        label={t("drift.code.tabWidth.title")}
                        items={codeTabWidths.map((width) => ({
                            id: String(width),
                            label: t("drift.code.spaces", { count: width }),
                        }))}
                        selected={String(codeTabWidth())}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="9rem"
                        onPick={(value) => setCodeTabWidth(Number(value))}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.code.wordWrap.title")}
                    description={t("drift.code.wordWrap.description")}
                    onClick={() => setCodeWordWrap(!codeWordWrap())}
                >
                    <Toggle
                        label={t("drift.code.wordWrap.title")}
                        checked={codeWordWrap()}
                        onChange={() => setCodeWordWrap(!codeWordWrap())}
                    />
                </SettingsRow>
            </SettingsGroup>
            <SettingsGroup title={t("drift.code.diffs")}>
                <SettingsRow
                    title={t("drift.code.diffWordWrap.title")}
                    description={t("drift.code.diffWordWrap.description")}
                    onClick={() => setDiffWordWrap(!diffWordWrap())}
                >
                    <Toggle
                        label={t("drift.code.diffWordWrap.title")}
                        checked={diffWordWrap()}
                        onChange={() => setDiffWordWrap(!diffWordWrap())}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.code.lineNumbers.title")}
                    description={t("drift.code.lineNumbers.description")}
                    onClick={() => setDiffLineNumbers(!diffLineNumbers())}
                >
                    <Toggle
                        label={t("drift.code.lineNumbers.title")}
                        checked={diffLineNumbers()}
                        onChange={() => setDiffLineNumbers(!diffLineNumbers())}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.code.diffIndicator.title")}
                    description={t("drift.code.diffIndicator.description")}
                >
                    <Picker
                        label={t("drift.code.diffIndicator.title")}
                        items={diffIndicators.map((name) => ({ id: name, label: t(diffIndicatorLabels[name]) }))}
                        selected={diffIndicator()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="10rem"
                        onPick={(value) => setDiffIndicator(value as DiffIndicator)}
                    />
                </SettingsRow>
            </SettingsGroup>
        </div>
    );
}
