import { setSpeechVoice, speechVoices } from "../state/voice";
import { createMemo, createResource } from "solid-js";
import { Picker, type PickerItem } from "./picker";
import { SettingsRow } from "./settings-controls";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { components } from "../engine/native/types";

type SpeechVoice = components["schemas"]["SpeechVoice"];

const groups: SpeechVoice["group"][] = ["custom", "community", "workspace", "default"];
const picker = { floating: true, bordered: true, chevronAtEnd: true, placement: "below", width: "16rem" } as const;

/** ElevenLabs' voices for the account behind the saved key, asked again whenever the key changes. */
export function ElevenLabsVoiceRow(props: { keyed?: string | null }) {
    const engine = useEngine();
    const [voices] = createResource(
        () => props.keyed ?? undefined,
        () => engine.actions.speechVoices("elevenlabs"),
    );
    const chosen = () => speechVoices().elevenlabs ?? "";
    const items = createMemo<PickerItem[]>(() => {
        const listed = [...(voices.error ? [] : (voices() ?? []))].sort(
            (a, b) => groups.indexOf(a.group) - groups.indexOf(b.group) || a.name.localeCompare(b.name),
        );
        const kept = chosen() && !listed.some((voice) => voice.id === chosen());

        return [
            { id: "", label: t("common.default") },
            ...(kept ? [{ id: chosen(), label: chosen() }] : []),
            ...listed.map((voice) => ({
                id: voice.id,
                label: voice.name,
                detail: voice.detail ?? undefined,
                group: t(`drift.voice.read.voice.group.${voice.group}`),
            })),
        ];
    });
    const description = () => {
        if (!props.keyed) return t("drift.voice.read.voice.needsKey");
        if (voices.loading) return t("drift.voice.read.voice.loading");
        if (voices.error)
            return t("drift.voice.read.voice.failed", {
                reason: voices.error instanceof Error ? voices.error.message : String(voices.error),
            });

        return t("drift.voice.read.voice.library");
    };

    return (
        <SettingsRow title={t("drift.voice.read.voice.title")} description={description()}>
            <Picker
                label={t("drift.voice.read.voice.title")}
                items={items()}
                selected={chosen()}
                {...picker}
                onPick={(value) => setSpeechVoice("elevenlabs", value)}
            />
        </SettingsRow>
    );
}
