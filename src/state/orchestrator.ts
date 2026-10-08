/**
 * Notices for sessions running the `orchestrator` agent. The engine drives the agent itself
 * (`crates/drift-engine/src/session/drive.rs`): it keeps a turn going while the agent's status
 * block says `working`. The UI only reports how a driven turn ended.
 */

export const ORCHESTRATOR_AGENT = "orchestrator";

/** The engine's `drive::MAX_ROUNDS`: nudges it sends per prompt of the user's own. */
export const ORCHESTRATOR_MAX_ROUNDS = 30;

type EntryPart = { type: string; synthetic?: boolean; metadata?: Record<string, unknown> };

/** Nudges since the user's newest prompt of their own (text they typed, or a file), as the engine counts them. */
export function nudgesSincePrompt(entries: Array<{ info: { role: string }; parts: EntryPart[] }>) {
    let count = 0;
    for (const entry of [...entries].reverse()) {
        if (entry.info.role !== "user") continue;
        if (
            entry.parts.some(
                (part) => part.type === "file" || (part.type === "text" && !part.synthetic && !part.metadata),
            )
        )
            return count;
        if (entry.parts.some((part) => part.metadata?.generated === true)) count++;
    }
    return count;
}

export type OrchestratorState = "working" | "done" | "blocked";
export type OrchestratorStatus = { state: OrchestratorState; headline?: string };

const statusBlock = /<orchestrator_status>\s*([\s\S]*?)\s*<\/orchestrator_status>/g;
// A block still streaming in has no closing tag yet; it is hidden until it does.
const openBlock = /<orchestrator_status>[\s\S]*$/;

/** A reply's prose without its status blocks, and the status the final one states, for showing apart. */
export function splitOrchestratorStatus(text: string): { prose: string; status?: OrchestratorStatus } {
    const prose = text.replace(statusBlock, "").replace(openBlock, "").trimEnd();
    return { prose, status: parseOrchestratorStatus(text) };
}

/** Parses the final status block of a reply; the last one wins. Anything invalid is undefined. */
export function parseOrchestratorStatus(text: string | undefined): OrchestratorStatus | undefined {
    if (!text) return undefined;
    const last = [...text.matchAll(statusBlock)].at(-1);
    if (!last) return undefined;
    // The protocol puts the block last, so trailing prose means the reply did not follow it.
    if (text.slice(last.index + last[0].length).trim()) return undefined;
    const raw = last[1];
    if (!raw) return undefined;
    try {
        const parsed = JSON.parse(raw) as { state?: unknown; headline?: unknown };
        if (parsed.state !== "working" && parsed.state !== "done" && parsed.state !== "blocked") return undefined;
        return {
            state: parsed.state,
            ...(typeof parsed.headline === "string" && parsed.headline.trim()
                ? { headline: parsed.headline.trim().slice(0, 200) }
                : {}),
        };
    } catch {
        return undefined;
    }
}

export type OrchestratorEndInput = {
    /** The status the session just left; only busy/retry -> idle edges are turn endings. */
    previousStatus?: string;
    status: string;
    agent?: string;
    /** Subagent sessions are the orchestrator's workers and never driven. */
    parentID?: string;
    lastMessage?: { role: string; completed: boolean; errored: boolean; text: string };
    /** Nudges the engine sent since the user's own prompt. */
    rounds: number;
};

export type OrchestratorNotice = { title: string; message: string; variant: "success" | "warning" };

/**
 * How a driven turn ended, as a notice; null when there is nothing to say. A clean reply that
 * still says `working` names the round limit only when the nudges reached it; otherwise a Stop
 * or a refused nudge ended the turn, and the user already knows or sees why.
 */
export function orchestratorNotice(input: OrchestratorEndInput): OrchestratorNotice | null {
    if (!orchestratorTurnEnded(input)) return null;

    const last = input.lastMessage;
    if (!last || last.role !== "assistant" || !last.completed || last.errored) return null;
    const status = parseOrchestratorStatus(last.text);
    return orchestratorEndNotice(status, input.rounds);
}

function orchestratorTurnEnded(input: OrchestratorEndInput) {
    if (input.agent !== ORCHESTRATOR_AGENT || input.parentID || input.status !== "idle") return false;

    return input.previousStatus === "busy" || input.previousStatus === "retry";
}

function orchestratorEndNotice(status: OrchestratorStatus | undefined, rounds: number): OrchestratorNotice | null {
    if (status?.state === "done")
        return {
            title: "Orchestrator finished",
            message: status.headline ?? "The goal was reported complete.",
            variant: "success",
        };
    if (status?.state === "blocked")
        return {
            title: "Orchestrator blocked",
            message: status.headline ?? "The orchestrator needs your input to continue.",
            variant: "warning",
        };
    if (rounds < ORCHESTRATOR_MAX_ROUNDS) return null;
    return {
        title: "Orchestrator paused",
        message: "The round limit was reached for this goal. Send a message to keep going.",
        variant: "warning",
    };
}
