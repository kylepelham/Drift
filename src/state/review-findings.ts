import { createSignal } from "solid-js";

import type { FindingOutcome, ReviewFinding } from "../engine/native/client";
import type { MessageEntry } from "../engine/store";
import type { ToolPart } from "../engine/parts";

/** One `report_findings` call; each finding's id is the call's part id and its place in the list. */
export type FindingReport = { part: ToolPart; level?: string; findings: (ReviewFinding & { id: string })[] };
/** Where a finding sits in a file's merged lines: on its line, on the same text elsewhere, or on a line since changed. */
export type FindingAnchor = { index: number; state: "exact" | "moved" | "changed" };

/** The engine records at most this many characters of a finding's line. */
const recordedChars = 400;

/** Every finished `report_findings` call in a transcript, oldest first. */
export function findingReports(entries: MessageEntry[]): FindingReport[] {
    const reports: FindingReport[] = [];
    for (const entry of entries) {
        for (const part of entry.parts) {
            if (part.type !== "tool_call" || part.name !== "report_findings" || part.status !== "done") continue;
            const findings = (part.metadata?.findings ?? []).map((finding, at) => ({
                ...finding,
                id: `${part.id}:${at}`,
            }));
            reports.push({ part, level: part.metadata?.level ?? undefined, findings });
        }
    }

    return reports;
}

/** A report that only records what became of earlier findings, which the card of those findings shows. */
export function isOutcomeReport(findings: ReviewFinding[]) {
    return findings.length > 0 && findings.every((finding) => !!finding.outcome);
}

const findingKey = (finding: ReviewFinding) => `${finding.file}\n${finding.summary}`;
const lineKey = (finding: ReviewFinding) => `${finding.file}\n${finding.line ?? ""}`;

/** Each finding's outcome: its own, else the one the latest later report gives the same finding. */
export function findingOutcomes(reports: FindingReport[], partId: string): (FindingOutcome | undefined)[] {
    const at = reports.findIndex((report) => report.part.id === partId);
    if (at < 0) return [];

    const bySummary = new Map<string, FindingOutcome>();
    const byLine = new Map<string, FindingOutcome>();
    for (const report of reports.slice(at + 1)) {
        for (const finding of report.findings) {
            if (!finding.outcome) continue;
            bySummary.set(findingKey(finding), finding.outcome);
            byLine.set(lineKey(finding), finding.outcome);
        }
    }

    return reports[at].findings.map(
        (finding) => finding.outcome ?? bySummary.get(findingKey(finding)) ?? byLine.get(lineKey(finding)),
    );
}

/** Whether a line as it reads now is the one a finding recorded, allowing for the engine's clip. */
function sameCode(text: string, code: string) {
    const clipped = [...text].slice(0, recordedChars).join("");

    return clipped.trimEnd() === code.trimEnd();
}

/**
 * Where `finding` sits in a file's merged lines (each prefixed ` `, `+` or `-`): its line when the text still matches,
 * else the nearest line with its text, else its line number marked as changed. `undefined` for a file with no lines.
 */
export function anchorFinding(finding: ReviewFinding, lines: string[]): FindingAnchor | undefined {
    const current = lines.flatMap((line, index) => (line[0] === "-" ? [] : [index]));
    if (!current.length) return undefined;

    const wanted = finding.line ?? 1;
    const at = Math.min(wanted, current.length) - 1;
    const text = (number: number) => lines[current[number]].slice(1);
    const code = finding.code;
    if (code === undefined || code === null) return { index: current[at], state: "exact" };
    if (wanted <= current.length && sameCode(text(at), code)) return { index: current[at], state: "exact" };

    for (let distance = 1; distance < current.length; distance++) {
        for (const number of [at - distance, at + distance]) {
            if (number >= 0 && number < current.length && sameCode(text(number), code))
                return { index: current[number], state: "moved" };
        }
    }

    return { index: current[at], state: "changed" };
}

/** The prompt Apply fixes sends: each open finding, then what to do and how to report it. */
export function fixPrompt(intro: string, findings: ReviewFinding[]) {
    const listed = findings.map((finding, at) => {
        const place = finding.line ? `${finding.file}:${finding.line}` : finding.file;
        const scenario = finding.failureScenario ? `\n   ${finding.failureScenario}` : "";
        return `${at + 1}. ${place} - ${finding.summary}${scenario}`;
    });

    return [intro, ...listed].join("\n\n");
}

/** Findings the user dismissed in the review panel, by id, for as long as the app runs. */
const [dismissed, setDismissed] = createSignal<ReadonlySet<string>>(new Set());

export const dismissedFindings = dismissed;

export function dismissFinding(id: string) {
    setDismissed(new Set([...dismissed(), id]));
}
