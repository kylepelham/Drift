import { SettingsGroup, SettingsRow } from "./settings-controls";
import { openExternal, shellInvoke } from "../shell";
import { createSignal, Show } from "solid-js";
import { Jellyfish } from "./jellyfish";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

const websiteUrl = "https://driftagent.dev";
const [updateSupported, setUpdateSupported] = createSignal<boolean | undefined>();
let updateSupportLoad: Promise<void> | undefined;

export function preloadUpdateSupport() {
    if (updateSupportLoad) return;

    const invoke = shellInvoke();
    if (!invoke) return setUpdateSupported(false);

    updateSupportLoad = invoke<boolean>("update_support")
        .then((supported) => {
            setUpdateSupported(supported === true);
        })
        .catch(() => {
            setUpdateSupported(false);
        });
}

export function AboutSection() {
    const engine = useEngine();
    const nativeVersion = () => {
        if (!engine.state.nativeVersion)
            return engine.state.startupError ? t("drift.about.failed") : t("drift.about.starting");

        const link = engine.state.nativeOnline ? t("drift.about.native.connected") : t("drift.about.native.offline");

        return `${engine.state.nativeVersion} (${link})`;
    };

    return (
        <div class="space-y-6 select-text">
            <div class="flex flex-col items-center gap-2 text-center">
                <Jellyfish class="size-32" />
                <div class="drift-wordmark text-base">drift</div>
                <p class="max-w-xs text-[0.76rem] leading-relaxed text-ink-muted">{t("drift.about.description")}</p>
            </div>

            <SettingsGroup title={t("drift.about.group.build")}>
                <SettingsRow title={t("drift.about.row.app.title")} description={t("drift.about.row.app.description")}>
                    <span class="font-mono text-[0.75rem] text-ink-muted">{__DRIFT_VERSION__}</span>
                </SettingsRow>
                <SettingsRow
                    title={t("drift.about.row.native.title")}
                    description={t("drift.about.row.native.description")}
                >
                    <span class="font-mono text-[0.75rem] text-ink-muted">{nativeVersion()}</span>
                </SettingsRow>
                <SettingsRow
                    title={t("drift.about.row.updates.title")}
                    description={t(updateSupportLabel(updateSupported(), true))}
                >
                    <span class="text-[0.75rem] text-ink-muted">{t(updateSupportLabel(updateSupported(), false))}</span>
                </SettingsRow>
                <Show when={engine.state.startupError}>
                    <div class="px-1 py-2.5 text-[0.72rem] leading-relaxed text-danger">
                        {engine.state.startupError}
                    </div>
                </Show>
            </SettingsGroup>

            <SettingsGroup title={t("drift.about.group.links")}>
                <SettingsRow
                    title={t("drift.about.row.website.title")}
                    description={t("drift.about.row.website.description")}
                >
                    <button
                        class="rounded border border-edge px-2 py-1 text-[0.72rem] text-accent hover:bg-raised"
                        onClick={() => openExternal(websiteUrl)}
                    >
                        driftagent.dev
                    </button>
                </SettingsRow>
            </SettingsGroup>
        </div>
    );
}

function updateSupportLabel(supported: boolean | undefined, description: boolean) {
    if (supported === undefined) return description ? "drift.about.starting" : "common.loading";
    if (description) return supported ? "drift.about.row.updates.installed" : "drift.about.row.updates.local";

    return supported ? "drift.about.updates.available" : "drift.about.updates.unavailable";
}
