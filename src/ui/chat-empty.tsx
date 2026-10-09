import { activeWorkspace } from "../state/workspaces";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";
import { Show } from "solid-js";

/** The chat pane with no thread selected: the logo and where to start. */
export function EmptyState() {
    return (
        <div class="flex h-full flex-col items-center justify-center gap-3 select-none">
            <DriftLogo class="fade-up size-16 text-ink" label="Drift" />
            <Show
                when={activeWorkspace()}
                fallback={
                    <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
                        {t("drift.chat.empty.noWorkspace")}
                    </div>
                }
            >
                <div class="fade-up text-sm text-ink-muted" style={{ "animation-delay": "80ms" }}>
                    {t("drift.chat.empty.promptHint")}
                </div>
            </Show>
            <div class="fade-up text-xs text-ink-faint" style={{ "animation-delay": "160ms" }}>
                {t("drift.chat.empty.threadHint")}
            </div>
        </div>
    );
}
