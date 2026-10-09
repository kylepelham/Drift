import { SettingsGroup, SettingsRow } from "./settings-controls";
import { requestNotificationPermission } from "./notifications";
import { createMemo, createSignal, For } from "solid-js";
import { playAlertSound, soundOptions } from "./sounds";
import { readDataUrl } from "./files";
import { Toggle } from "./controls";
import { IconPlus } from "./icons";
import { t } from "../state/i18n";
import { Picker } from "./picker";
import {
    alertSounds,
    attentionKinds,
    customSound,
    setAlertSound,
    setCustomSound,
    setSystemNotification,
    systemNotifications,
} from "../state/prefs";

import type { AttentionKind } from "../state/prefs";

const notificationKeys: Record<AttentionKind, string> = {
    agent: "agent",
    permission: "permissions",
    error: "errors",
};

export function NotificationsSection() {
    return (
        <div class="space-y-5">
            <SettingsGroup title={t("settings.general.section.notifications")}>
                <For each={attentionKinds}>
                    {(kind) => {
                        const toggle = () => {
                            const next = !systemNotifications()[kind];
                            if (next) requestNotificationPermission();

                            setSystemNotification(kind, next);
                        };

                        return (
                            <SettingsRow
                                title={t(`settings.general.notifications.${notificationKeys[kind]}.title`)}
                                description={t(`settings.general.notifications.${notificationKeys[kind]}.description`)}
                                onClick={toggle}
                            >
                                <Toggle
                                    label={t(`settings.general.notifications.${notificationKeys[kind]}.title`)}
                                    checked={!!systemNotifications()[kind]}
                                    onChange={toggle}
                                />
                            </SettingsRow>
                        );
                    }}
                </For>
            </SettingsGroup>

            <SettingsGroup title={t("settings.general.section.sounds")}>
                <For each={attentionKinds}>
                    {(kind) => (
                        <SettingsRow
                            title={t(`settings.general.sounds.${notificationKeys[kind]}.title`)}
                            description={t(`settings.general.sounds.${notificationKeys[kind]}.description`)}
                        >
                            <SoundPicker kind={kind} />
                        </SettingsRow>
                    )}
                </For>
            </SettingsGroup>
        </div>
    );
}

function SoundPicker(props: { kind: AttentionKind }) {
    let picker!: HTMLInputElement;
    const [error, setError] = createSignal("");
    const options = createMemo(() => {
        const sound = customSound();
        const customOptions = sound
            ? [{ id: "custom", label: `${t("prompt.slash.badge.custom")}: ${sound.name}` }]
            : [];

        return [
            { id: "none", label: t("sound.option.none") },
            ...soundOptions.map((item) => ({ id: item.id, label: item.label, group: item.group })),
            ...customOptions,
        ];
    });

    async function upload(file: File | undefined) {
        if (!file) return;
        if (!file.type.startsWith("audio/")) return setError(t("drift.settings.sound.audioFileRequired"));
        if (file.size > 1024 * 1024) return setError(t("drift.settings.sound.maxSize"));

        const dataUrl = await readDataUrl(file);
        const sound = { name: file.name, dataUrl };

        setCustomSound(sound);
        setAlertSound(props.kind, "custom");
        setError("");
        void playAlertSound("custom", sound);
    }

    return (
        <div class="flex min-w-0 items-center gap-1.5" title={error() || undefined}>
            <Picker
                label={`${t(`settings.general.sounds.${notificationKeys[props.kind]}.title`)} ${t("settings.general.section.sounds")}`}
                items={options()}
                selected={alertSounds()[props.kind] ?? "none"}
                floating
                bordered
                chevronAtEnd
                placement="below"
                width="9.5rem"
                onPick={(id) => {
                    setAlertSound(props.kind, id);
                    void playAlertSound(id, customSound());
                }}
            />
            <input
                ref={picker}
                type="file"
                accept="audio/*,.aac,.mp3,.wav,.ogg,.m4a"
                class="hidden"
                onChange={(event) => {
                    void upload(event.currentTarget.files?.[0]);
                    event.currentTarget.value = "";
                }}
            />
            <button
                title={t("drift.settings.sound.chooseCustom")}
                class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                onClick={() => picker.click()}
            >
                <IconPlus class="size-3.5" />
            </button>
        </div>
    );
}
