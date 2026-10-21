import { autoReadReply, readingReply, toggleReadAloud } from "../voice/read-aloud";
import { messageText, sessionBusy, type MessageEntry } from "../engine/store";
import { splitOrchestratorStatus } from "../state/orchestrator";
import { selectedSession } from "../state/selection";
import { messageProblem } from "../engine/messages";
import { readAloudEnabled } from "../state/voice";
import { createEffect, on, Show } from "solid-js";
import { IconSpeaker, IconStop } from "./icons";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

function replyProse(entry: MessageEntry) {
    return splitOrchestratorStatus(messageText(entry)).prose;
}

function useSpeech() {
    const engine = useEngine();
    const failed = (cause: unknown) =>
        engine.actions.notice({ message: cause instanceof Error ? cause.message : String(cause), variant: "error" });

    return { engine, failed };
}

/** Reads a reply when the auto-read switch is on and it finishes in the open conversation; never one loaded finished. */
export function createAutoRead(entry: () => MessageEntry, footer: () => boolean) {
    const { engine, failed } = useSpeech();
    const done = () => {
        const info = entry().info;
        return footer() && !!info.finishedAt && !sessionBusy(engine.state, info.sessionId) && !messageProblem(info);
    };

    createEffect(
        on(done, (now, before) => {
            const info = entry().info;
            if (!now || before !== false || selectedSession() !== info.sessionId) return;
            void autoReadReply(info.id, replyProse(entry()), engine.actions.speakText).catch(failed);
        }),
    );
}

/** The play button on a finished reply; it stops the reading when pressed again. */
export function ReadAloudButton(props: { entry: MessageEntry }) {
    const { engine, failed } = useSpeech();
    const reading = () => readingReply() === props.entry.info.id;
    const label = () => (reading() ? t("drift.voice.read.stop") : t("drift.voice.read.play"));
    const toggle = () =>
        void toggleReadAloud(props.entry.info.id, replyProse(props.entry), engine.actions.speakText).catch(failed);

    return (
        <Show when={readAloudEnabled()}>
            <button
                title={label()}
                aria-label={label()}
                aria-pressed={reading()}
                class="rounded p-0.5 hover:bg-raised hover:text-ink"
                classList={{ "text-ink": reading() }}
                onClick={toggle}
            >
                <Show when={reading()} fallback={<IconSpeaker class="size-3.5" />}>
                    <IconStop class="size-3.5" />
                </Show>
            </button>
        </Show>
    );
}
