import { corners, customCss, setCorners, setCustomCss, setUiFont, theme, uiFont } from "../state/theme";
import { interfaceZoom, maxZoom, minZoom, setInterfaceZoom } from "../state/zoom";
import { AccentPicker, CustomColors, ThemeGallery } from "./settings-theme";
import { SettingsGroup, SettingsRow } from "./settings-controls";
import { defaultFonts, FontPicker } from "./settings-font";
import { Segmented, Slider, Toggle } from "./controls";
import { ThinkingSettings } from "./settings-thinking";
import { createSignal, Show } from "solid-js";
import { firstFamily } from "../state/fonts";
import { t } from "../state/i18n";
import {
    composerScale,
    conversationScale,
    iconScale,
    interfaceTextScale,
    maxIconScale,
    maxTextScale,
    minTextScale,
    setComposerScale,
    setConversationScale,
    setIconScale,
    setInterfaceTextScale,
    setSettingsScale,
    setSidebarScale,
    settingsScale,
    sidebarScale,
} from "../state/text-size";
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
    backdropBlur,
    backdropDim,
    backdropFromFile,
    backdropImage,
    setBackdropBlur,
    setBackdropDim,
    setBackdropImage,
} from "../state/backdrop";

import type { SplashExitAnimation, SplashMascotAnimation } from "../state/startup";
import type { CornerStyle } from "../state/theme";

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
                <ThemeGallery />
                <Show when={theme() === "drift-custom"}>
                    <CustomColors />
                </Show>
            </SettingsGroup>

            <SettingsGroup title={t("drift.accent.title")}>
                <div class="space-y-2 py-3">
                    <div class="text-[length:calc(0.75rem*var(--settings-scale,1))] leading-relaxed text-ink-muted">
                        {t("drift.accent.description")}
                    </div>
                    <AccentPicker />
                </div>
            </SettingsGroup>

            <PersonalizeGroup />

            <SettingsGroup title={t("drift.settings.typography")}>
                <SettingsRow
                    title={t("settings.general.row.uiFont.title")}
                    description={t("settings.general.row.uiFont.description")}
                >
                    <FontPicker
                        label={t("settings.general.row.uiFont.title")}
                        value={uiFont()}
                        onInput={setUiFont}
                        fallback={defaultFonts.ui}
                    />
                </SettingsRow>
            </SettingsGroup>

            <SettingsGroup title={t("drift.textSize.title")}>
                <SizeRow
                    title={t("drift.textSize.interface")}
                    description={t("drift.textSize.interface.description")}
                    min={minZoom}
                    max={maxZoom}
                    step={0.1}
                    value={interfaceZoom()}
                    onInput={setInterfaceZoom}
                    settleOnRelease
                />
                <SizeRow
                    title={t("drift.textSize.ui")}
                    description={t("drift.textSize.ui.description")}
                    value={interfaceTextScale()}
                    onInput={setInterfaceTextScale}
                />
                <SizeRow
                    title={t("drift.textSize.conversation")}
                    description={t("drift.textSize.conversation.description")}
                    value={conversationScale()}
                    onInput={setConversationScale}
                />
                <SizeRow
                    title={t("drift.textSize.sidebar")}
                    description={t("drift.textSize.sidebar.description")}
                    value={sidebarScale()}
                    onInput={setSidebarScale}
                />
                <SizeRow
                    title={t("drift.textSize.composer")}
                    description={t("drift.textSize.composer.description")}
                    value={composerScale()}
                    onInput={setComposerScale}
                />
                <SizeRow
                    title={t("drift.textSize.settings")}
                    description={t("drift.textSize.settings.description")}
                    value={settingsScale()}
                    onInput={setSettingsScale}
                    settleOnRelease
                />
                <SizeRow
                    title={t("drift.textSize.icons")}
                    description={t("drift.textSize.icons.description")}
                    max={maxIconScale}
                    value={iconScale()}
                    onInput={setIconScale}
                />
            </SettingsGroup>

            <ThinkingSettings />

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
                        <Segmented
                            label={t("startup.settings.mascot.title")}
                            options={splashMascotAnimations.map((name) => ({
                                id: name,
                                label: t(mascotAnimationLabels[name]),
                            }))}
                            value={splashMascotAnimation()}
                            onChange={(value) => setSplashMascotAnimation(value as SplashMascotAnimation)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.exit.title")}
                        description={t("startup.settings.exit.description")}
                    >
                        <Segmented
                            label={t("startup.settings.exit.title")}
                            options={splashExitAnimations.map((name) => ({
                                id: name,
                                label: t(exitAnimationLabels[name]),
                            }))}
                            value={splashExitAnimation()}
                            onChange={(value) => setSplashExitAnimation(value as SplashExitAnimation)}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.duration.title")}
                        description={t("startup.settings.duration.description")}
                    >
                        <Segmented
                            label={t("startup.settings.duration.title")}
                            options={splashDurations.map((duration) => ({
                                id: String(duration),
                                label: t(durationLabels[duration]),
                            }))}
                            value={String(splashDuration())}
                            onChange={(value) => setSplashDuration(Number(value))}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("startup.settings.font.title")}
                        description={t("startup.settings.font.description")}
                    >
                        <FontPicker
                            label={t("startup.settings.font.title")}
                            value={splashFont()}
                            onInput={setSplashFont}
                            fallback={uiFont() ? firstFamily(uiFont()) : defaultFonts.ui}
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

/** A size as a slider with its percentage, dragged back to 100% for the default. `settleOnRelease` waits for the
 * drag to end before applying, for a size that rescales the slider under the pointer. */
function SizeRow(props: {
    title: string;
    description: string;
    min?: number;
    max?: number;
    step?: number;
    value: number;
    onInput: (value: number) => void;
    settleOnRelease?: boolean;
}) {
    const [dragged, setDragged] = createSignal<number>();
    const shown = () => dragged() ?? props.value;

    return (
        <SettingsRow title={props.title} description={props.description}>
            <div class="flex items-center gap-3">
                <Slider
                    label={props.title}
                    min={props.min ?? minTextScale}
                    max={props.max ?? maxTextScale}
                    step={props.step ?? 0.05}
                    value={shown()}
                    onInput={props.onInput}
                    onDragging={props.settleOnRelease ? setDragged : undefined}
                />
                <button
                    class="w-11 rounded-md py-0.5 text-right text-xs text-ink-muted tabular-nums transition-colors hover:text-ink"
                    title={t("drift.textSize.reset")}
                    onClick={() => props.onInput(1)}
                >
                    {Math.round(shown() * 100)}%
                </button>
            </div>
        </SettingsRow>
    );
}

/** A picture behind the conversation and the corner style. */
function PersonalizeGroup() {
    const [failed, setFailed] = createSignal("");
    let picker!: HTMLInputElement;

    const choose = async (file: File | undefined) => {
        if (!file) return;
        setFailed("");
        try {
            const image = await backdropFromFile(file);
            if (!setBackdropImage(image)) setFailed(t("drift.backdrop.tooLarge"));
        } catch (cause) {
            setFailed(t("drift.backdrop.failed", { reason: cause instanceof Error ? cause.message : String(cause) }));
        }
    };
    const cornerOptions: { id: CornerStyle; label: string }[] = [
        { id: "sharp", label: t("drift.corners.sharp") },
        { id: "", label: t("drift.corners.default") },
        { id: "round", label: t("drift.corners.round") },
    ];

    return (
        <SettingsGroup title={t("drift.personalize.title")}>
            <SettingsRow title={t("drift.backdrop.title")} description={failed() || t("drift.backdrop.description")}>
                <div class="flex items-center gap-2">
                    <Show when={backdropImage()}>
                        {(image) => (
                            <span
                                class="h-9 w-14 shrink-0 rounded-md border border-edge bg-cover bg-center"
                                style={{ "background-image": `url("${image()}")` }}
                            />
                        )}
                    </Show>
                    <input
                        ref={picker}
                        type="file"
                        accept="image/*"
                        class="hidden"
                        onChange={(event) => {
                            void choose(event.currentTarget.files?.[0]);
                            event.currentTarget.value = "";
                        }}
                    />
                    <button class={settingsButton} onClick={() => picker.click()}>
                        {backdropImage() ? t("drift.backdrop.change") : t("drift.backdrop.choose")}
                    </button>
                    <Show when={backdropImage()}>
                        <button class={settingsButton} onClick={() => setBackdropImage("")}>
                            {t("drift.backdrop.remove")}
                        </button>
                    </Show>
                </div>
            </SettingsRow>
            <Show when={backdropImage()}>
                <SettingsRow title={t("drift.backdrop.dim")} description={t("drift.backdrop.dim.description")}>
                    <Slider
                        label={t("drift.backdrop.dim")}
                        min={0.05}
                        max={1}
                        step={0.05}
                        value={1 - backdropDim()}
                        onInput={(strength) => setBackdropDim(1 - strength)}
                    />
                </SettingsRow>
                <SettingsRow title={t("drift.backdrop.blur")} description={t("drift.backdrop.blur.description")}>
                    <Slider
                        label={t("drift.backdrop.blur")}
                        min={0}
                        max={24}
                        step={1}
                        value={backdropBlur()}
                        onInput={setBackdropBlur}
                    />
                </SettingsRow>
            </Show>
            <SettingsRow title={t("drift.corners.title")} description={t("drift.corners.description")}>
                <Segmented
                    label={t("drift.corners.title")}
                    options={cornerOptions}
                    value={corners()}
                    onChange={setCorners}
                />
            </SettingsRow>
        </SettingsGroup>
    );
}

const settingsButton =
    "h-8 rounded-md border border-edge bg-raised/50 px-3 text-xs font-medium text-ink-muted transition-colors hover:border-edge-strong hover:text-ink";
