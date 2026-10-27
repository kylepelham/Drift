import { createMemo, createSignal, For, Show } from "solid-js";
import { savedChoice, sessionBusy } from "../engine/store";
import { defaultBase } from "./review-sources";
import { openReview } from "../state/panels";
import { FileIcon } from "./file-icon";
import { useEngine } from "../engine";
import { t } from "../state/i18n";
import {
    dismissedFindings,
    findingOutcomes,
    findingReports,
    fixPrompt,
    reviewedFiles,
    type FindingReport,
} from "../state/review-findings";

import type { FindingOutcome } from "../engine/native/client";
import type { ToolPart } from "../engine/parts";

type Engine = ReturnType<typeof useEngine>;
type Finding = FindingReport["findings"][number];

export const outcomeLabels: Record<FindingOutcome, string> = {
    fixed: "drift.findings.outcome.fixed",
    skipped: "drift.findings.outcome.skipped",
    no_change_needed: "drift.findings.outcome.noChangeNeeded",
};

const tallyLabels: Record<FindingOutcome, string> = {
    fixed: "drift.findings.tally.fixed",
    skipped: "drift.findings.tally.skipped",
    no_change_needed: "drift.findings.tally.noChangeNeeded",
};

/** Opens the review panel at the findings: on the files as they are for a codebase review, else on the branch's changes. */
async function walkFindings(engine: Engine, sessionId: string, partId: string, current?: string, files?: string[]) {
    const findings = { sessionId, partId, current };
    if (files) return openReview({ source: { kind: "files", target: files.join("\n") }, findings });

    const directory = engine.state.sessions[sessionId]?.directory;
    const refs = await engine.actions.reviewRefs(directory).catch(() => undefined);
    const base = defaultBase(refs);
    const source = base ? { kind: "branch" as const, target: base } : { kind: "uncommitted" as const };

    openReview({ source, findings });
}

/** Sends the findings as a prompt to fix them, after the running turn when there is one. */
export async function applyFindingFixes(engine: Engine, sessionId: string, findings: Finding[]) {
    const choice = savedChoice(engine.state, sessionId);

    await engine.actions.send(sessionId, fixPrompt(t("drift.findings.applyPrompt"), findings), {
        model: choice.model ?? null,
        agent: choice.agent ?? "build",
        variant: choice.variant,
        queue: sessionBusy(engine.state, sessionId),
    });
}

function OutcomeBadge(props: { outcome?: FindingOutcome }) {
    return (
        <Show when={props.outcome}>
            {(outcome) => (
                <span
                    class="shrink-0 text-xs"
                    classList={{ "text-ok": outcome() === "fixed", "text-ink-faint": outcome() !== "fixed" }}
                >
                    {t(outcomeLabels[outcome()])}
                </span>
            )}
        </Show>
    );
}

function FindingRow(props: { finding: Finding; outcome?: FindingOutcome; onOpen: () => void }) {
    const tooltip = () => [props.finding.summary, props.finding.failureScenario].filter(Boolean).join("\n\n");

    return (
        <button
            class="flex w-full min-w-0 items-center gap-2 rounded-md py-1 pr-2 pl-8 text-left transition-colors hover:bg-raised/45"
            title={tooltip()}
            onClick={() => props.onOpen()}
        >
            <span class="min-w-0 truncate" classList={{ "text-ink-faint line-through": props.outcome === "fixed" }}>
                {props.finding.shortSummary ?? props.finding.summary}
            </span>
            <Show when={props.finding.line}>
                {(line) => <span class="shrink-0 font-mono text-xs text-ink-faint">:{line()}</span>}
            </Show>
            <Show when={props.finding.verdict === "plausible"}>
                <span class="shrink-0 text-xs text-ink-faint">{t("drift.findings.plausible")}</span>
            </Show>
            <span class="flex-1" />
            <OutcomeBadge outcome={props.outcome} />
        </button>
    );
}

/** Findings by file, in the order the review ranked them, each with its place in the report. */
function byFile(findings: Finding[]) {
    const files = new Map<string, { finding: Finding; at: number }[]>();
    findings.forEach((finding, at) => files.set(finding.file, [...(files.get(finding.file) ?? []), { finding, at }]));

    return [...files.entries()].map(([file, items]) => ({ file, items }));
}

/** The card a code review leaves in the transcript: its findings by file, what became of each, and what to do next. */
export function FindingsCard(props: { part: ToolPart }) {
    const engine = useEngine();
    const sessionId = () => props.part.sessionId;
    const reports = createMemo(() => findingReports(engine.state.transcripts[sessionId()] ?? []));
    const findings = createMemo(() => reports().find((report) => report.part.id === props.part.id)?.findings ?? []);
    const outcomes = createMemo(() => findingOutcomes(reports(), props.part.id));
    const open = () => findings().filter((finding, at) => !outcomes()[at] && !dismissedFindings().has(finding.id));
    const busy = () => sessionBusy(engine.state, sessionId());
    const [sending, setSending] = createSignal(false);
    const codebase = () => props.part.metadata?.scope === "codebase";

    const heading = () => {
        const count = findings().length;
        const counted = count
            ? t(count === 1 ? "drift.findings.count.one" : "drift.findings.count.other", { count })
            : t("drift.findings.none");
        const level = props.part.metadata?.level;
        const title = t(codebase() ? "drift.findings.titleCodebase" : "drift.findings.title");
        return [title, level, counted].filter(Boolean).join(" · ");
    };
    const tally = () =>
        (Object.keys(tallyLabels) as FindingOutcome[])
            .map((outcome) => ({ outcome, count: outcomes().filter((found) => found === outcome).length }))
            .filter((entry) => entry.count > 0)
            .map((entry) => t(tallyLabels[entry.outcome], { count: entry.count }))
            .join(" · ");

    const walk = (current?: string) =>
        void walkFindings(
            engine,
            sessionId(),
            props.part.id,
            current,
            codebase() ? reviewedFiles(findings()) : undefined,
        );
    const apply = async () => {
        setSending(true);
        await applyFindingFixes(engine, sessionId(), open());
        setSending(false);
    };
    const rerun = () => {
        const scoped = codebase() ? props.part.metadata?.path : undefined;
        const args = [props.part.metadata?.level, scoped].filter(Boolean).join(" ");
        void engine.actions.runCommand(sessionId(), codebase() ? "audit" : "review", args);
    };

    return (
        <div class="findings-card rounded-lg border border-edge/70 p-1.5 text-sm">
            <div class="flex min-w-0 items-center gap-2 px-2 py-1">
                <span class="min-w-0 truncate text-ink-muted">{heading()}</span>
                <Show when={tally()}>{(text) => <span class="shrink-0 text-xs text-ink-faint">{text()}</span>}</Show>
            </div>
            <For each={byFile(findings())}>
                {(group) => (
                    <>
                        <div
                            class="flex min-w-0 items-center gap-2 px-2 pt-1 text-xs text-ink-faint"
                            title={group.file}
                        >
                            <FileIcon path={group.file} class="size-4" />
                            <span class="min-w-0 truncate">{group.file}</span>
                        </div>
                        <For each={group.items}>
                            {(item) => (
                                <FindingRow
                                    finding={item.finding}
                                    outcome={outcomes()[item.at]}
                                    onOpen={() => walk(item.finding.id)}
                                />
                            )}
                        </For>
                    </>
                )}
            </For>
            <div class="flex flex-wrap items-center gap-1.5 px-2 pt-1.5 pb-0.5">
                <Show when={findings().length}>
                    <button
                        class="rounded-md border border-edge px-2 py-1 text-xs transition-colors hover:bg-raised"
                        onClick={() => walk()}
                    >
                        {t("drift.findings.walk")}
                    </button>
                </Show>
                <Show when={open().length}>
                    <button
                        class="rounded-md border border-edge px-2 py-1 text-xs transition-colors hover:bg-raised disabled:opacity-50"
                        disabled={sending()}
                        onClick={() => void apply()}
                    >
                        {t("drift.findings.apply")}
                    </button>
                </Show>
                <button
                    class="rounded-md px-2 py-1 text-xs text-ink-faint transition-colors hover:bg-raised hover:text-ink disabled:opacity-50"
                    disabled={busy()}
                    onClick={rerun}
                >
                    {t("drift.findings.rerun")}
                </button>
            </div>
        </div>
    );
}
