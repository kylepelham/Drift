import { SettingsGroup, SettingsRow } from "./settings-controls";
import { FontField } from "./settings-font";
import { For, Show } from "solid-js";
import { Toggle } from "./controls";
import { IconCheck } from "./icons";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    setSplashDuration,
    setSplashEnabled,
    setSplashExitAnimation,
    setSplashFont,
    setSplashMascotAnimation,
    splashDuration,
    splashDurations,
    splashEnabled,
    splashExitAnimation,
    splashExitAnimations,
    splashFont,
    splashMascotAnimation,
    splashMascotAnimations,
} from "../state/startup";
import {
    customCss,
    customTheme,
    setCustomCss,
    setCustomThemeColor,
    setTheme,
    setUiFont,
    theme,
    themes,
    uiFont,
} from "../state/theme";

import type { SplashExitAnimation, SplashMascotAnimation } from "../state/startup";
import type { CustomTheme, ThemeName } from "../state/theme";

export const themeMeta: Record<ThemeName, { label: string; swatch: [string, string, string] }> = {
    "drift-dark": { label: "drift.theme.dark", swatch: ["#141517", "#212429", "#7ba3e8"] },
    "drift-graphite": { label: "drift.theme.graphite", swatch: ["#101112", "#222326", "#b7b9c2"] },
    "drift-midnight": { label: "drift.theme.midnight", swatch: ["#0c1020", "#19223a", "#8aa8ff"] },
    "drift-slate": { label: "drift.theme.slate", swatch: ["#0f1419", "#1b232c", "#6cb2c9"] },
    "drift-forest": { label: "drift.theme.forest", swatch: ["#0f1512", "#1d2922", "#82c99a"] },
    "drift-aubergine": { label: "drift.theme.aubergine", swatch: ["#171119", "#2d2031", "#d29ad8"] },
    "drift-light": { label: "drift.theme.light", swatch: ["#f4f4f5", "#ffffff", "#3a6fd8"] },
    "drift-paper": { label: "drift.theme.paper", swatch: ["#eee9df", "#fffdf8", "#97643c"] },
    "drift-custom": { label: "drift.theme.custom", swatch: ["#111318", "#1b1e25", "#a78bfa"] },
};

const customColorMeta: { id: keyof CustomTheme; label: string }[] = [
    { id: "background", label: "drift.color.background" },
    { id: "surface", label: "drift.color.surface" },
    { id: "text", label: "drift.color.text" },
    { id: "accent", label: "drift.color.accent" },
];
const mascotAnimationLabels: Record<SplashMascotAnimation, string> = {
    bounce: "startup.settings.mascot.bounce",
    float: "startup.settings.mascot.float",
    pulse: "startup.settings.mascot.pulse",
    still: "startup.settings.mascot.still",
};
const exitAnimationLabels: Record<SplashExitAnimation, string> = {
    wave: "startup.settings.exit.wave",
    fade: "startup.settings.exit.fade",
    lift: "startup.settings.exit.lift",
};
const durationLabels: Record<number, string> = {
    1500: "startup.settings.duration.brief",
    3200: "startup.settings.duration.balanced",
    5000: "startup.settings.duration.extended",
};

export function AppearanceSection() {
    return (
        <div class="space-y-6">
            <SettingsGroup title={t("settings.general.row.theme.title")}>
                <div class="space-y-0.5 py-1">
                    <For each={themes}>{(name) => <ThemeRow name={name} />}</For>
                </div>
            </SettingsGroup>

            <Show when={theme() === "drift-custom"}>
                <SettingsGroup title={t("drift.settings.customPalette")}>
                    <For each={customColorMeta}>
                        {(color) => (
                            <SettingsRow
                                title={t(color.label)}
                                description={t("drift.settings.customPalette.colorDescription", {
                                    color: t(color.label).toLowerCase(),
                                })}
                            >
                                <div class="flex items-center gap-2">
                                    <input
                                        type="color"
                                        aria-label={t("dialog.project.edit.color.select", { color: t(color.label) })}
                                        class="size-7 cursor-pointer rounded border border-edge bg-transparent p-0.5"
                                        value={customTheme()[color.id]}
                                        onInput={(event) => setCustomThemeColor(color.id, event.currentTarget.value)}
                                    />
                                    <input
                                        aria-label={t("drift.settings.customPalette.hexValue", {
                                            color: t(color.label),
                                        })}
                                        class="h-8 w-24 rounded-md border border-edge bg-raised/45 px-2 font-mono text-xs text-ink outline-none focus:border-accent"
                                        maxLength={7}
                                        pattern="#[0-9a-fA-F]{6}"
                                        value={customTheme()[color.id]}
                                        onChange={(event) => {
                                            if (/^#[\da-f]{6}$/i.test(event.currentTarget.value))
                                                setCustomThemeColor(color.id, event.currentTarget.value);
                                        }}
                                    />
                                </div>
                            </SettingsRow>
                        )}
                    </For>
                </SettingsGroup>
            </Show>

            <SettingsGroup title={t("drift.settings.typography")}>
                <SettingsRow
                    title={t("settings.general.row.uiFont.title")}
                    description={t("settings.general.row.uiFont.description")}
                >
                    <FontField label={t("settings.general.row.uiFont.title")} value={uiFont()} onInput={setUiFont} />
                </SettingsRow>
            </SettingsGroup>

            <SettingsGroup title={t("startup.settings.title")}>
                <SettingsRow
                    title={t("startup.settings.show.title")}
                    description={t("startup.settings.show.description")}
                    onClick={() => setSplashEnabled(!splashEnabled())}
                >
                    <Toggle
                        label={t("startup.settings.show.title")}
                        checked={splashEnabled()}
                        onChange={() => setSplashEnabled(!splashEnabled())}
                    />
                </SettingsRow>
                <Show when={splashEnabled()}>
                    <SettingsRow
                        title={t("startup.settings.mascot.title")}
                        description={t("startup.settings.mascot.description")}
                    >
                        <Picker
                            label={t("startup.settings.mascot.title")}
                            items={splashMascotAnimations.map((name) => ({
                                id: name,
                                label: t(mascotAnimationLabels[name]),
                            }))}
                            selected={splashMascotAnimation()}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width="10rem"
                            onPick={(value) => setSplashMascotAnimation(value as SplashMascotAnimation)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.exit.title")}
                        description={t("startup.settings.exit.description")}
                    >
                        <Picker
                            label={t("startup.settings.exit.title")}
                            items={splashExitAnimations.map((name) => ({
                                id: name,
                                label: t(exitAnimationLabels[name]),
                            }))}
                            selected={splashExitAnimation()}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width="10rem"
                            onPick={(value) => setSplashExitAnimation(value as SplashExitAnimation)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.duration.title")}
                        description={t("startup.settings.duration.description")}
                    >
                        <Picker
                            label={t("startup.settings.duration.title")}
                            items={splashDurations.map((duration) => ({
                                id: String(duration),
                                label: t(durationLabels[duration]),
                            }))}
                            selected={String(splashDuration())}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width="11rem"
                            onPick={(value) => setSplashDuration(Number(value))}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.font.title")}
                        description={t("startup.settings.font.description")}
                    >
                        <FontField
                            label={t("startup.settings.font.title")}
                            value={splashFont()}
                            onInput={setSplashFont}
                        />
                    </SettingsRow>
                </Show>
            </SettingsGroup>

            <SettingsGroup title={t("drift.settings.customCss")}>
                <div class="py-2">
                    <div class="mb-2 text-xs leading-relaxed text-ink-faint">
                        {t("drift.settings.customCss.description")}
                    </div>
                    <textarea
                        aria-label={t("drift.settings.customCss")}
                        class="h-32 w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed text-ink outline-none placeholder:text-ink-faint focus:border-accent"
                        placeholder=":root { --accent: #8aa8ff; }"
                        spellcheck={false}
                        value={customCss()}
                        onInput={(event) => setCustomCss(event.currentTarget.value)}
                    />
                </div>
            </SettingsGroup>
        </div>
    );
}

function ThemeRow(props: { name: ThemeName }) {
    const meta = () => themeMeta[props.name];
    const active = () => theme() === props.name;
    const swatch = () =>
        props.name === "drift-custom"
            ? ([customTheme().background, customTheme().surface, customTheme().accent] as [string, string, string])
            : meta().swatch;

    return (
        <button
            class="flex w-full items-center gap-3 rounded-lg border px-3 py-2 text-left transition-colors"
            classList={{
                "border-edge-strong bg-raised": active(),
                "border-transparent hover:bg-raised/60": !active(),
            }}
            onClick={() => setTheme(props.name)}
        >
            <span class="flex items-center">
                <For each={swatch()}>
                    {(color, index) => (
                        <span
                            class="-ml-1.5 size-4 rounded-full border border-black/30 first:ml-0"
                            style={{ background: color, "z-index": 3 - index() }}
                        />
                    )}
                </For>
            </span>
            <span
                class="min-w-0 flex-1 truncate text-sm"
                classList={{ "text-ink": active(), "text-ink-muted": !active() }}
            >
                {t(meta().label)}
            </span>
            <Show when={active()}>
                <IconCheck class="size-4 shrink-0 text-accent" />
            </Show>
        </button>
    );
}
