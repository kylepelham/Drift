import { IconHourglass } from "./icons";
import { t } from "../state/i18n";

import type { BackgroundWork } from "../engine/store";

/** "2 workers, 1 command": the background work still running, empty when none is counted. */
export function backgroundWorkText(work: BackgroundWork) {
    const workers = t(work.workers === 1 ? "drift.stop.workers.one" : "drift.stop.workers.other", {
        count: work.workers,
    });
    const commands = t(work.commands === 1 ? "drift.stop.commands.one" : "drift.stop.commands.other", {
        count: work.commands,
    });
    if (work.workers && work.commands) return t("drift.stop.both", { workers, commands });
    if (work.workers) return workers;
    return work.commands ? commands : "";
}

/** "Waiting on 1 command", or "Waiting" for a wait whose work is not counted yet. */
function waitingText(work: BackgroundWork) {
    const what = backgroundWorkText(work);
    return what ? t("drift.waiting.on", { work: what }) : t("drift.waiting");
}

/** In place of Thinking: the conversation does nothing but wait for background work to wake it. */
export function WaitingIndicator(props: { work: BackgroundWork }) {
    return (
        <div class="timeline-waiting select-none" role="status" aria-live="polite">
            <IconHourglass class="waiting-hourglass size-3.5 shrink-0" />
            <span>{waitingText(props.work)}</span>
        </div>
    );
}

/** The sidebar's mark for the same state: an hourglass, never the working dot. */
export function WaitingMark(props: { work: BackgroundWork }) {
    return (
        <span class="flex shrink-0 text-ink-muted" title={waitingText(props.work)}>
            <IconHourglass class="waiting-hourglass size-3" />
        </span>
    );
}
