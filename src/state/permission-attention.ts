import { childrenOf, sessionBusy, workerQueued, type EngineState } from "../engine/store";

/** Published asks need user attention because the engine already handles auto-accepted requests. */

/** A session is waiting on the user when it has an unanswered question or permission. */
export function sessionNeedsAttention(state: EngineState, id: string) {
    return (state.questions[id]?.length ?? 0) > 0 || (state.permissions[id]?.length ?? 0) > 0;
}

/** A subagent row in the sidebar and how deep it sits: 0 under the thread, 1 under one of its subagents, and so on. */
type SidebarWorker = { session: ReturnType<typeof childrenOf>[number]; depth: number };

/** Subagents at any depth appear while they run, wait on the user, or are open, with the chain above them; finished work lives in the task card. */
export function sidebarWorkers(state: EngineState, parentId: string, selected?: string | null): SidebarWorker[] {
    return childrenOf(state, parentId).flatMap((child) => {
        const nested = sidebarWorkers(state, child.id, selected).map((worker) => ({
            ...worker,
            depth: worker.depth + 1,
        }));
        const shown =
            nested.length > 0 ||
            child.id === selected ||
            sessionBusy(state, child.id) ||
            workerQueued(state, child.id) ||
            sessionNeedsAttention(state, child.id);
        return shown ? [{ session: child, depth: 0 }, ...nested] : [];
    });
}
