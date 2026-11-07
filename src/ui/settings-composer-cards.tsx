import { SettingsGroup, SettingsRow } from "./settings-controls";
import { Segmented, Toggle } from "./controls";
import { t } from "../state/i18n";
import { For } from "solid-js";
import {
    composerCardKinds,
    composerCards,
    composerCardStyle,
    composerCardStyles,
    setComposerCard,
    setComposerCardStyle,
} from "../state/prefs";

/** Settings > General > Above the composer: how its cards sit, and which ones show. */
export function ComposerCardsSettings() {
    return (
        <SettingsGroup title={t("drift.settings.composerCards")}>
            <SettingsRow
                title={t("drift.settings.composerCards.style.title")}
                description={t("drift.settings.composerCards.style.description")}
            >
                <Segmented
                    label={t("drift.settings.composerCards.style.title")}
                    options={composerCardStyles.map((style) => ({
                        id: style,
                        label: t(`drift.settings.composerCards.style.${style}`),
                    }))}
                    value={composerCardStyle()}
                    onChange={setComposerCardStyle}
                />
            </SettingsRow>
            <For each={composerCardKinds}>
                {(kind) => (
                    <SettingsRow
                        title={t(`drift.settings.composerCards.${kind}.title`)}
                        description={t(`drift.settings.composerCards.${kind}.description`)}
                        onClick={() => setComposerCard(kind, !composerCards()[kind])}
                    >
                        <Toggle
                            label={t(`drift.settings.composerCards.${kind}.title`)}
                            checked={composerCards()[kind]}
                            onChange={() => setComposerCard(kind, !composerCards()[kind])}
                        />
                    </SettingsRow>
                )}
            </For>
        </SettingsGroup>
    );
}
