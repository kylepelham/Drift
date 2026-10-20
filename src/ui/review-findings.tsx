import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { IconChevronLeft, IconChevronRight, IconX } from "./icons";
import { applyFindingFixes, outcomeLabels } from "./findings-card";
import { noteBoxClass } from "./review-comment";
import { samePath } from "./review-sources";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    anchorFinding,
    dismissedFindings,
    dismissFinding,
    findingOutcomes,
    findingReports,
    type FindingAnchor,
    type FindingReport,
} from "../state/review-findings";

import type { FindingOutcome, ReviewFile } from "../engine/native/client";
import type { ReviewPanel } from "../state/panels";

export type WalkFinding = FindingReport["findings"][number] & { outcome?: FindingOutcome };
export type AnchoredFinding = { finding: WalkFinding; path: string; anchor: FindingAnchor };
/** The findings a review panel walks through, the one it is on, and how to move. */
export type FindingsWalk = {
    sessionId: () => string | undefined;
    findings: () => WalkFinding[];
    current: () => WalkFinding | undefined;
    select: (id: string | undefined) => void;
    /** True once after the walk moves to a finding, so its note scrolls into view once rather than on every rebuild. */
    claimSeek: () => boolean;
};

/** The panel's findings that are not dismissed, each with its outcome, starting at the one it was opened on. */
export function createFindingsWalk(target: () => ReviewPanel["findings"]): FindingsWalk {
    const engine = useEngine();
    const all = createMemo(() => {
        const found = target();
        if (!found) return [];

        const reports = findingReports(engine.state.transcripts[found.sessionId] ?? []);
        const outcomes = findingOutcomes(reports, found.partId);
        const report = reports.find((item) => item.part.id === found.partId);
        return (report?.findings ?? []).map((finding, at) => ({ ...finding, outcome: outcomes[at] }));
    });
    const findings = createMemo(() => all().filter((finding) => !dismissedFindings().has(finding.id)));
    const [chosen, setChosen] = createSignal<string>();
    let due = true;
    createEffect(
        on(target, (found) => {
            due = true;
            setChosen(found?.current);
        }),
    );
    const current = createMemo(() => findings().find((finding) => finding.id === chosen()) ?? findings()[0]);
    const select = (id: string | undefined) => {
        if (id !== current()?.id) due = true;
        setChosen(id);
    };
    const claimSeek = () => {
        const claimed = due;
        due = false;
        return claimed;
    };

    return { sessionId: () => target()?.sessionId, findings, current, select, claimSeek };
}

/** Each finding placed on its file's lines in this review, by finding id; findings outside it are left out. */
export function anchorFindings(findings: WalkFinding[], files: ReviewFile[]) {
    const anchored = new Map<string, AnchoredFinding>();
    for (const finding of findings) {
        const file = files.find((item) => item.preview === "lines" && samePath(item.path, finding.file));
        const anchor = file && anchorFinding(finding, file.lines);
        if (file && anchor) anchored.set(finding.id, { finding, path: file.path, anchor });
    }

    return anchored;
}

const stepButton =
    "flex size-6 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors enabled:hover:bg-raised enabled:hover:text-ink disabled:opacity-40";
const textButton =
    "shrink-0 rounded px-1.5 py-0.5 text-ink-faint transition-colors enabled:hover:bg-raised enabled:hover:text-ink disabled:opacity-40";

/** Finding N of M with the one it is on, previous and next, Apply fixes for every open finding, and Dismiss. */
export function FindingsStepper(props: {
    walk: FindingsWalk;
    anchored: Map<string, AnchoredFinding>;
    onClose: () => void;
}) {
    const engine = useEngine();
    const [sending, setSending] = createSignal(false);
    const position = () => props.walk.findings().findIndex((finding) => finding.id === props.walk.current()?.id);
    const step = (by: number) => props.walk.select(props.walk.findings()[position() + by]?.id);
    const open = () => props.walk.findings().filter((finding) => !finding.outcome);
    const place = (finding: WalkFinding) => (finding.line ? `${finding.file}:${finding.line}` : finding.file);

    const apply = async () => {
        const sessionId = props.walk.sessionId();
        if (!sessionId) return;

        setSending(true);
        await applyFindingFixes(engine, sessionId, open());
        setSending(false);
    };
    const dismiss = () => {
        const current = props.walk.current();
        const next = props.walk.findings()[position() + 1] ?? props.walk.findings()[position() - 1];
        if (!current) return;

        props.walk.select(next?.id);
        dismissFinding(current.id);
    };

    return (
        <div class="flex shrink-0 flex-col gap-1 border-b border-edge/60 px-4 py-2 text-xs">
            <div class="flex min-w-0 items-center gap-1">
                <span class="shrink-0 text-ink-muted">
                    {t("drift.findings.step", { index: position() + 1, count: props.walk.findings().length })}
                </span>
                <button
                    class={stepButton}
                    title={t("drift.findings.previous")}
                    aria-label={t("drift.findings.previous")}
                    disabled={position() <= 0}
                    onClick={() => step(-1)}
                >
                    <IconChevronLeft class="size-3.5" />
                </button>
                <button
                    class={stepButton}
                    title={t("drift.findings.next")}
                    aria-label={t("drift.findings.next")}
                    disabled={position() >= props.walk.findings().length - 1}
                    onClick={() => step(1)}
                >
                    <IconChevronRight class="size-3.5" />
                </button>
                <span class="flex-1" />
                <Show when={open().length}>
                    <button class={textButton} disabled={sending()} onClick={() => void apply()}>
                        {t("drift.findings.apply")}
                    </button>
                </Show>
                <button class={textButton} onClick={dismiss}>
                    {t("drift.findings.dismiss")}
                </button>
                <button
                    class={stepButton}
                    title={t("drift.findings.close")}
                    aria-label={t("drift.findings.close")}
                    onClick={() => props.onClose()}
                >
                    <IconX class="size-3.5" />
                </button>
            </div>
            <Show when={props.walk.current()}>
                {(finding) => (
                    <div class="flex min-w-0 items-center gap-2" title={finding().summary}>
                        <span class="shrink-0 font-mono text-ink-faint">{place(finding())}</span>
                        <span class="min-w-0 truncate text-ink">{finding().shortSummary ?? finding().summary}</span>
                        <Show when={!props.anchored.has(finding().id)}>
                            <span class="shrink-0 text-ink-faint">{t("drift.findings.notInDiff")}</span>
                        </Show>
                    </div>
                )}
            </Show>
        </div>
    );
}

/** A finding under its line: what fails and how, and a warning when the line moved or changed since the review. */
export function FindingNote(props: {
    item: AnchoredFinding;
    current: boolean;
    claimSeek: () => boolean;
    onSelect: () => void;
}) {
    let root!: HTMLDivElement;
    const finding = () => props.item.finding;
    const tags = () => {
        const outcome = finding().outcome;
        const verdict = finding().verdict === "plausible" ? t("drift.findings.plausible") : undefined;

        return [finding().category, verdict, outcome && t(outcomeLabels[outcome])].filter(Boolean).join(" · ");
    };
    // Its chunk is built on demand when it becomes current, so the note scrolls itself into view once it is on the page.
    createEffect(() => {
        if (props.current && props.claimSeek()) queueMicrotask(() => root.scrollIntoView({ block: "center" }));
    });

    return (
        <div ref={root} class={noteBoxClass} data-finding-id={finding().id}>
            <button
                class="w-full rounded-lg border bg-surface px-3 py-2 text-left"
                classList={{ "border-accent/70": props.current, "border-edge": !props.current }}
                onClick={() => props.onSelect()}
            >
                <Show when={tags()}>
                    {(text) => (
                        <div class="text-[length:calc(0.7rem*var(--ui-text-scale,1))] text-ink-faint">{text()}</div>
                    )}
                </Show>
                <div class="mt-0.5 text-[length:calc(0.82rem*var(--ui-text-scale,1))] whitespace-pre-wrap text-ink [overflow-wrap:anywhere]">
                    {finding().summary}
                </div>
                <Show when={finding().failureScenario}>
                    <div class="mt-1 text-[length:calc(0.78rem*var(--ui-text-scale,1))] whitespace-pre-wrap text-ink-muted [overflow-wrap:anywhere]">
                        {finding().failureScenario}
                    </div>
                </Show>
                <Show when={props.item.anchor.state !== "exact" && finding().outcome !== "fixed"}>
                    <div class="mt-1 text-[length:calc(0.7rem*var(--ui-text-scale,1))] text-warn">
                        {t(props.item.anchor.state === "moved" ? "drift.findings.moved" : "drift.findings.changed")}
                    </div>
                </Show>
            </button>
        </div>
    );
}
