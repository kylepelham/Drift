import { filePreviewPrefs, setFilePreviewMode, setFilePreviewType } from "../state/file-preview-prefs";
import { language, languages, setLanguage } from "../state/language";
import { SettingsGroup, SettingsRow } from "./settings-controls";
import { createSignal, For, onMount, Show } from "solid-js";
import { filePreviewTypes } from "../file-preview-types";
import { isRemoteRuntime } from "../runtime";
import { useEngine } from "../engine";
import { Toggle } from "./controls";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    animateResponses,
    autoUpdate,
    collapseCompaction,
    compactionCollapsed,
    responseAnimationSpeed,
    responseAnimationSpeedMax,
    responseAnimationSpeedMin,
    setAnimateResponses,
    setAutoUpdate,
    setCollapseCompaction,
    setCompactionCollapsed,
    setResponseAnimationSpeed,
    setShowReasoning,
    setSidebarDayDividers,
    setToolErrorsExpanded,
    showReasoning,
    sidebarDayDividers,
    toolErrorsExpanded,
} from "../state/prefs";

import type { LanguageId } from "../state/language";

export function GeneralSection() {
    const engine = useEngine();
    // The toggle stays disabled until the engine returns its preference.
    const [autoCompact, setAutoCompactShown] = createSignal<boolean | null>(null);

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => setAutoCompactShown(settings.autoCompact ?? true))
                .catch(() => undefined),
    );

    function toggleAutoCompact() {
        const next = !autoCompact();
        setAutoCompactShown(next);

        void engine.actions
            .setAutoCompact(next)
            .then((settings) => setAutoCompactShown(settings.autoCompact ?? next))
            .catch(() => setAutoCompactShown(!next));
    }

    return (
        <div class="space-y-5">
            <SettingsGroup title={t("settings.general.section.display")}>
                <SettingsRow
                    title={t("settings.general.row.language.title")}
                    description={t("settings.general.row.language.description")}
                >
                    <Picker
                        label={t("settings.general.row.language.title")}
                        items={languages.map((item) => ({ id: item.id, label: item.label }))}
                        selected={language()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="12rem"
                        onPick={(value) => setLanguage(value as LanguageId)}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.dayDividers.title")}
                    description={t("drift.settings.dayDividers.description")}
                    onClick={() => setSidebarDayDividers(!sidebarDayDividers())}
                >
                    <Toggle
                        label={t("drift.settings.dayDividers.title")}
                        checked={sidebarDayDividers()}
                        onChange={() => setSidebarDayDividers(!sidebarDayDividers())}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.responseAnimation.title")}
                    description={t("drift.settings.responseAnimation.description")}
                    onClick={() => setAnimateResponses(!animateResponses())}
                >
                    <Toggle
                        label={t("drift.settings.responseAnimation.title")}
                        checked={animateResponses()}
                        onChange={() => setAnimateResponses(!animateResponses())}
                    />
                </SettingsRow>
                <Show when={animateResponses()}>
                    <SettingsRow
                        title={t("drift.settings.responseAnimation.speed.title")}
                        description={t("drift.settings.responseAnimation.speed.description")}
                    >
                        <div class="flex items-center gap-2.5">
                            <input
                                id="response-reveal-speed"
                                type="range"
                                class="response-speed-slider"
                                min={responseAnimationSpeedMin}
                                max={responseAnimationSpeedMax}
                                step="12"
                                value={responseAnimationSpeed()}
                                aria-label={t("drift.settings.responseAnimation.speed.title")}
                                aria-valuetext={t("drift.settings.responseAnimation.speed.value", {
                                    speed: responseAnimationSpeed(),
                                })}
                                onInput={(event) => setResponseAnimationSpeed(event.currentTarget.valueAsNumber)}
                            />
                            <output
                                for="response-reveal-speed"
                                class="w-7 text-right text-[0.68rem] tabular-nums text-ink-faint"
                                title={t("drift.settings.responseAnimation.speed.value", {
                                    speed: responseAnimationSpeed(),
                                })}
                            >
                                {responseAnimationSpeed()}
                            </output>
                        </div>
                    </SettingsRow>
                </Show>
            </SettingsGroup>

            <SettingsGroup title={t("drift.preview.settings.title")}>
                <SettingsRow
                    title={t("drift.preview.settings.mode")}
                    description={t("drift.preview.settings.description")}
                >
                    <Picker
                        label={t("drift.preview.settings.mode")}
                        items={(["all", "none", "custom"] as const).map((mode) => ({
                            id: mode,
                            label: t(`drift.preview.mode.${mode}`),
                        }))}
                        selected={filePreviewPrefs().mode}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width="12rem"
                        onPick={(mode) => {
                            if (mode === "all" || mode === "none" || mode === "custom") setFilePreviewMode(mode);
                        }}
                    />
                </SettingsRow>
                <Show when={filePreviewPrefs().mode === "custom"}>
                    <For each={filePreviewTypes}>
                        {(type) => (
                            <SettingsRow
                                title={t(`drift.preview.type.${type}`)}
                                description=""
                                onClick={() => setFilePreviewType(type, !filePreviewPrefs().types[type])}
                            >
                                <Toggle
                                    label={t(`drift.preview.type.${type}`)}
                                    checked={filePreviewPrefs().types[type]}
                                    onChange={() => setFilePreviewType(type, !filePreviewPrefs().types[type])}
                                />
                            </SettingsRow>
                        )}
                    </For>
                </Show>
            </SettingsGroup>

            <SettingsGroup title={t("settings.agents.title")}>
                <SettingsRow
                    title={t("command.permissions.autoaccept.enable")}
                    description={t("toast.permissions.autoaccept.on.description")}
                    onClick={() => void engine.actions.setAutoAcceptAll(!engine.state.autoAcceptAll)}
                >
                    <Toggle
                        label={t("command.permissions.autoaccept.enable")}
                        checked={engine.state.autoAcceptAll}
                        onChange={() => void engine.actions.setAutoAcceptAll(!engine.state.autoAcceptAll)}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("settings.general.row.reasoningSummaries.title")}
                    description={t("settings.general.row.reasoningSummaries.description")}
                    onClick={() => setShowReasoning(!showReasoning())}
                >
                    <Toggle
                        label={t("settings.general.row.reasoningSummaries.title")}
                        checked={showReasoning()}
                        onChange={() => setShowReasoning(!showReasoning())}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.toolErrors.title")}
                    description={t("drift.settings.toolErrors.description")}
                    onClick={() => setToolErrorsExpanded(!toolErrorsExpanded())}
                >
                    <Toggle
                        label={t("drift.settings.toolErrors.title")}
                        checked={toolErrorsExpanded()}
                        onChange={() => setToolErrorsExpanded(!toolErrorsExpanded())}
                    />
                </SettingsRow>
            </SettingsGroup>

            <SettingsGroup title={t("drift.settings.summaries")}>
                <SettingsRow
                    title={t("drift.settings.autoCompact.title")}
                    description={t("drift.settings.autoCompact.description")}
                    disabled={autoCompact() === null}
                    onClick={() => autoCompact() !== null && toggleAutoCompact()}
                >
                    <Toggle
                        label={t("drift.settings.autoCompact.title")}
                        checked={autoCompact() ?? false}
                        disabled={autoCompact() === null}
                        onChange={toggleAutoCompact}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.summaries.collapsible.title")}
                    description={t("drift.settings.summaries.collapsible.description")}
                    onClick={() => setCollapseCompaction(!collapseCompaction())}
                >
                    <Toggle
                        label={t("drift.settings.summaries.collapsible.title")}
                        checked={collapseCompaction()}
                        onChange={() => setCollapseCompaction(!collapseCompaction())}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.summaries.collapsed.title")}
                    description={t("drift.settings.summaries.collapsed.description")}
                    disabled={!collapseCompaction()}
                    onClick={() => collapseCompaction() && setCompactionCollapsed(!compactionCollapsed())}
                >
                    <Toggle
                        label={t("drift.settings.summaries.collapsed.title")}
                        checked={compactionCollapsed()}
                        disabled={!collapseCompaction()}
                        onChange={() => setCompactionCollapsed(!compactionCollapsed())}
                    />
                </SettingsRow>
            </SettingsGroup>

            <Show when={!isRemoteRuntime()}>
                <SettingsGroup title={t("settings.general.section.updates")}>
                    <SettingsRow
                        title={t("settings.updates.row.startup.title")}
                        description={t("settings.updates.row.startup.description")}
                        onClick={() => setAutoUpdate(!autoUpdate())}
                    >
                        <Toggle
                            label={t("settings.updates.row.startup.title")}
                            checked={autoUpdate()}
                            onChange={() => setAutoUpdate(!autoUpdate())}
                        />
                    </SettingsRow>
                </SettingsGroup>
            </Show>
        </div>
    );
}
