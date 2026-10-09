import { formatDictationElapsed } from "../voice/transcript";
import { t } from "../state/i18n";
import { Show } from "solid-js";
import {
    dictationActive,
    dictationElapsed,
    dictationError,
    dictationPending,
    dictationStatus,
    dismissDictationError,
} from "../voice/dictation";

/** Dictation under the draft: elapsed time and state while listening, or the error that stopped it. */
export function DictationStatus() {
    const voiceBusy = () => dictationActive() || dictationPending() > 0;

    const voiceHint = () => {
        if (dictationStatus() === "starting") return t("drift.voice.starting");
        return dictationPending() > 0 ? t("drift.voice.transcribing") : t("drift.voice.listening");
    };

    return (
        <Show when={voiceBusy() || dictationError()}>
            <div class="flex items-center gap-2 px-4 pt-2.5 text-xs">
                <Show
                    when={voiceBusy()}
                    fallback={
                        <button
                            class="min-w-0 truncate text-left text-danger hover:underline"
                            title={t("common.dismiss")}
                            onClick={dismissDictationError}
                        >
                            {dictationError()}
                        </button>
                    }
                >
                    <span class="size-1.5 shrink-0 animate-pulse rounded-full bg-danger" />
                    <Show when={dictationActive()}>
                        <span class="shrink-0 font-mono text-ink-faint">
                            {formatDictationElapsed(dictationElapsed())}
                        </span>
                    </Show>
                    <span class="min-w-0 truncate text-ink-faint italic">{voiceHint()}</span>
                </Show>
            </div>
        </Show>
    );
}
