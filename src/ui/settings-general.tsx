import { filePreviewPrefs, setFilePreviewMode, setFilePreviewType } from "../state/file-preview-prefs";
import { SettingsGroup, SettingsRow, pickerWidth } from "./settings-controls";
import { language, languages, setLanguage } from "../state/language";
import { createSignal, For, onMount, Show } from "solid-js";
import { filePreviewTypes } from "../file-preview-types";
import { compactionExamples } from "./model-details";
import { Segmented, Toggle } from "./controls";
import { isRemoteRuntime } from "../runtime";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    animateResponses,
    autoUpdate,
    collapseCompaction,
    commandRowModes,
    commandRowsFrom,
    commandRows,
    compactionCollapsed,
    editedFilesCard,
    responseAnimationSpeed,
    responseAnimationSpeedMax,
    responseAnimationSpeedMin,
    setAnimateResponses,
    setAutoUpdate,
    setCollapseCompaction,
    setCommandRows,
    setCompactionCollapsed,
    setEditedFilesCard,
    setResponseAnimationSpeed,
    setShowReasoning,
    setSidebarDayDividers,
    setToolErrorsExpanded,
    setToolGroup,
    showReasoning,
    sidebarDayDividers,
    toolErrorsExpanded,
    toolGroupKinds,
    toolGroups,
} from "../state/prefs";

import type { CompactAt } from "../engine/native/client";
import type { LanguageId } from "../state/language";

const compactPolicies: CompactAt[] = ["earlier", "auto", "later"];

/** The grouping control's label: none, all, or the chosen kinds by name. */
function groupedSummary() {
    const chosen = toolGroupKinds.filter((kind) => toolGroups()[kind]);
    if (chosen.length === 0) return t("drift.settings.groupTools.none");
    if (chosen.length === toolGroupKinds.length) return t("drift.settings.groupTools.all");

    return chosen.map((kind) => t(`drift.settings.groupTools.${kind}`)).join(", ");
}

export function GeneralSection() {
    const engine = useEngine();
    // The toggle stays disabled until the engine returns its preference.
    const [autoCompact, setAutoCompactShown] = createSignal<boolean | null>(null);
    const [compactAt, setCompactAtShown] = createSignal<CompactAt | null>(null);

    onMount(
        () =>
            void engine.actions
                .engineSettings()
                .then((settings) => {
                    setAutoCompactShown(settings.autoCompact ?? true);
                    setCompactAtShown(settings.compactAt ?? "auto");
                })
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

    function pickCompactAt(id: string) {
        const previous = compactAt();
        const next = id as CompactAt;
        setCompactAtShown(next);

        void engine.actions
            .setCompactAt(next)
            .then((settings) => setCompactAtShown(settings.compactAt ?? next))
            .catch(() => setCompactAtShown(previous));
    }

    /** The compaction description, with where the chosen policy puts the models in use. */
    function compactDescription() {
        const examples = compactionExamples(engine.state);
        const description = t("drift.settings.compactAt.description");
        if (!examples.length) return description;

        return `${description} ${t("drift.settings.compactAt.examples", { examples: examples.join(", ") })}`;
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
                        width={pickerWidth}
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
                                class="w-7 text-right text-[length:calc(0.68rem*var(--settings-scale,1))] tabular-nums text-ink-faint"
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

            <SettingsGroup title={t("drift.settings.transcript")}>
                <SettingsRow
                    title={t("drift.settings.collapseCommands.title")}
                    description={t("drift.settings.collapseCommands.description")}
                >
                    <Picker
                        label={t("drift.settings.collapseCommands.title")}
                        items={commandRowModes.map((mode) => ({
                            id: mode,
                            label: t(`drift.settings.collapseCommands.${mode}`),
                        }))}
                        selected={commandRows()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width={pickerWidth}
                        onPick={(mode) => setCommandRows(commandRowsFrom(mode))}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.groupTools.title")}
                    description={t("drift.settings.groupTools.description")}
                >
                    <Picker
                        label={t("drift.settings.groupTools.title")}
                        items={toolGroupKinds.map((kind) => ({
                            id: kind,
                            label: t(`drift.settings.groupTools.${kind}`),
                        }))}
                        checked={toolGroupKinds.filter((kind) => toolGroups()[kind])}
                        fallbackLabel={groupedSummary()}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width={pickerWidth}
                        onPick={(id) => {
                            const kind = toolGroupKinds.find((known) => known === id);
                            if (kind) setToolGroup(kind, !toolGroups()[kind]);
                        }}
                    />
                </SettingsRow>
                <SettingsRow
                    title={t("drift.settings.editedFiles.title")}
                    description={t("drift.settings.editedFiles.description")}
                    onClick={() => setEditedFilesCard(!editedFilesCard())}
                >
                    <Toggle
                        label={t("drift.settings.editedFiles.title")}
                        checked={editedFilesCard()}
                        onChange={() => setEditedFilesCard(!editedFilesCard())}
                    />
                </SettingsRow>
            </SettingsGroup>

            <SettingsGroup title={t("drift.preview.settings.title")}>
                <SettingsRow
                    title={t("drift.preview.settings.mode")}
                    description={t("drift.preview.settings.description")}
                >
                    <Segmented
                        label={t("drift.preview.settings.mode")}
                        options={(["all", "none", "custom"] as const).map((mode) => ({
                            id: mode,
                            label: t(`drift.preview.mode.${mode}`),
                        }))}
                        value={filePreviewPrefs().mode}
                        onChange={setFilePreviewMode}
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
                    title={t("drift.settings.compactAt.title")}
                    description={compactDescription()}
                    disabled={compactAt() === null}
                >
                    <Picker
                        label={t("drift.settings.compactAt.title")}
                        items={compactPolicies.map((id) => ({ id, label: t(`drift.settings.compactAt.${id}`) }))}
                        selected={compactAt() ?? "auto"}
                        floating
                        bordered
                        chevronAtEnd
                        placement="below"
                        width={pickerWidth}
                        onPick={pickCompactAt}
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
