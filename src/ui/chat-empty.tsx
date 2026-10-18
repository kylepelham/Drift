import { activeWorkspace } from "../state/workspaces";
import { DriftLogo } from "./logo";
import { t } from "../state/i18n";
import { Show } from "solid-js";

const quotes = 12;

/** The chat pane with no thread selected: the logo, where to start, and a line picked fresh each time. */
export function EmptyState() {
    const quote = `drift.chat.empty.quote${Math.floor(Math.random() * quotes) + 1}`;

    return (
        <div class="flex h-full flex-col items-center justify-center gap-3 select-none">
            <DriftLogo
                class="fade-up size-16 text-accent drop-shadow-[0_10px_18px_color-mix(in_srgb,var(--accent)_24%,transparent)]"
                label="Drift"
            />
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
                {t(quote)}
            </div>
        </div>
    );
}
