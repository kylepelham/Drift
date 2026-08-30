/**
 * The orchestrator driver: deterministic supervision for sessions running the `orchestrator`
 * agent. The agent must end every turn with a structured status block; this module parses it and
 * the driver acts on it - `working` auto-proceeds, `done` stops, `blocked` surfaces to the user.
 * A missing or malformed block gets a protocol reminder. No model ever judges another model.
 */

export const ORCHESTRATOR_AGENT = "orchestrator"

/** Driver turns allowed per user goal; each covers a full dispatch/verify batch. */
export const orchestratorMaxRounds = 30

export const PROCEED_PROMPT = [
  "Proceed toward the goal.",
  "Dispatch the next tasks now and verify results as they land.",
  "Do not re-summarize completed work.",
].join(" ")

export const STATUS_REMINDER_PROMPT = [
  "Your last reply did not end with a valid <orchestrator_status> block, so your state is unknown.",
  "Proceed toward the goal, and end every reply with the mandatory status block.",
].join(" ")

export type OrchestratorState = "working" | "done" | "blocked"
export type OrchestratorStatus = { state: OrchestratorState; headline?: string }

const statusBlock = /<orchestrator_status>\s*([\s\S]*?)\s*<\/orchestrator_status>/g

/** Parses the final status block of a reply; the last one wins. Anything invalid is undefined. */
export function parseOrchestratorStatus(text: string | undefined): OrchestratorStatus | undefined {
  if (!text) return undefined
  const last = [...text.matchAll(statusBlock)].at(-1)
  if (!last) return undefined
  // The protocol puts the block last, so trailing prose means the reply did not follow it.
  if (text.slice(last.index + last[0].length).trim()) return undefined
  const raw = last[1]
  if (!raw) return undefined
  try {
    const parsed = JSON.parse(raw) as { state?: unknown; headline?: unknown }
    if (parsed.state !== "working" && parsed.state !== "done" && parsed.state !== "blocked") return undefined
    return {
      state: parsed.state,
      ...(typeof parsed.headline === "string" && parsed.headline.trim()
        ? { headline: parsed.headline.trim().slice(0, 200) }
        : {}),
    }
  } catch {
    return undefined
  }
}

export type OrchestratorGateInput = {
  /** The status the session just left; only busy/retry -> idle edges are turn completions. */
  previousStatus?: string
  status: string
  goalAgent?: string
  /** Subagent sessions are the orchestrator's workers, driven by the engine, never by us. */
  parentID?: string
  pendingAsks: number
  lastMessage?: { role: string; completed: boolean; errored: boolean }
  rounds: number
}

/** Pure eligibility check; returns the blocking reason or null when the driver may act. */
export function orchestratorGate(input: OrchestratorGateInput): string | null {
  if (input.goalAgent !== ORCHESTRATOR_AGENT) return "not an orchestrator session"
  if (input.status !== "idle") return "not idle"
  if (input.previousStatus !== "busy" && input.previousStatus !== "retry") return "not a turn completion"
  if (input.parentID) return "subagent"
  if (input.pendingAsks > 0) return "awaiting permission or question"
  if (!input.lastMessage) return "no final message"
  if (input.lastMessage.role !== "assistant") return "no assistant reply"
  if (!input.lastMessage.completed) return "reply not completed"
  if (input.lastMessage.errored) return "reply errored"
  if (input.rounds >= orchestratorMaxRounds) return "round limit reached"
  return null
}

/** A user message written by Drift itself (proceed, reminders) never counts as a fresh goal. */
export function isGeneratedUserEntry(parts: Array<{ type: string; metadata?: Record<string, unknown> }>) {
  const text = parts.filter((part) => part.type === "text")
  return text.length > 0 && text.every((part) => part.metadata?.generated === true)
}
