import { createSignal } from "solid-js";

/** What the review panel compares: one turn's changes, or the workspace against git. */
export type ReviewSource =
    | { kind: "turn"; sessionId: string; messageId: string }
    | { kind: "uncommitted" }
    | { kind: "branch"; target: string }
    | { kind: "commit"; target: string };

export type ReviewPanel = {
    source: ReviewSource;
    /** The turn it was opened from, offered again in the source picker. */
    turn?: Extract<ReviewSource, { kind: "turn" }>;
    /** A file to scroll to once the review loads. */
    focus?: string;
    /** A `report_findings` call whose findings are walked through as notes on their lines, from `current` when set. */
    findings?: { sessionId: string; partId: string; current?: string };
};

type RightPanel = { kind: "debug" } | ({ kind: "review" } & ReviewPanel);

/** The debug and review panels share the slot right of the chat; opening one closes the other. */
const [rightPanel, setRightPanel] = createSignal<RightPanel | null>(null);

export function debugPanelOpen() {
    return rightPanel()?.kind === "debug";
}

export function setDebugPanelOpen(open: boolean) {
    if (open) setRightPanel({ kind: "debug" });
    else if (debugPanelOpen()) setRightPanel(null);
}

export function toggleDebugPanel() {
    setDebugPanelOpen(!debugPanelOpen());
}

export function reviewPanel(): ReviewPanel | undefined {
    const panel = rightPanel();
    return panel?.kind === "review" ? panel : undefined;
}

export function openReview(panel: ReviewPanel) {
    const turn = panel.source.kind === "turn" ? panel.source : (panel.turn ?? reviewPanel()?.turn);
    setRightPanel({ kind: "review", ...panel, turn });
}

export function closeReview() {
    if (reviewPanel()) setRightPanel(null);
}

export function rightPanelOpen() {
    return rightPanel() !== null;
}
