/**
 * Notices for sessions running the `orchestrator` agent. The engine drives the agent itself
 * (`crates/drift-engine/src/session/drive.rs`): it keeps a turn going while the agent's status
 * block says `working`. The UI only reports how a driven turn ended.
 */

export const ORCHESTRATOR_AGENT = "orchestrator"

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

export type OrchestratorEndInput = {
  /** The status the session just left; only busy/retry -> idle edges are turn endings. */
  previousStatus?: string
  status: string
  agent?: string
  /** Subagent sessions are the orchestrator's workers and never driven. */
  parentID?: string
  lastMessage?: { role: string; completed: boolean; errored: boolean; text: string }
}

export type OrchestratorNotice = { title: string; message: string; variant: "success" | "warning" }

/**
 * How a driven turn ended, as a notice; null when there is nothing to say. The engine ends a
 * clean turn that still says `working` (or has no valid status) only at its round limit.
 */
export function orchestratorNotice(input: OrchestratorEndInput): OrchestratorNotice | null {
  if (input.agent !== ORCHESTRATOR_AGENT || input.parentID || input.status !== "idle") return null
  if (input.previousStatus !== "busy" && input.previousStatus !== "retry") return null
  const last = input.lastMessage
  if (!last || last.role !== "assistant" || !last.completed || last.errored) return null
  const status = parseOrchestratorStatus(last.text)
  if (status?.state === "done")
    return { title: "Orchestrator finished", message: status.headline ?? "The goal was reported complete.", variant: "success" }
  if (status?.state === "blocked")
    return { title: "Orchestrator blocked", message: status.headline ?? "The orchestrator needs your input to continue.", variant: "warning" }
  return {
    title: "Orchestrator paused",
    message: "The round limit was reached for this goal. Send a message to keep going.",
    variant: "warning",
  }
}
