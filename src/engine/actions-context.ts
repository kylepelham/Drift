import { EngineError, type Client } from "./native/client";

import type { SetStoreFunction } from "solid-js/store";
import type { EngineState, Notice } from "./store";
import type { WorkspaceIndex } from "./sessions";

export type NoticeInput = Omit<Notice, "id" | "created" | "duration"> & {
    id?: string;
    created?: number;
    duration?: number;
};

/** What every group of engine actions works with: the client, the store and the workspace index. */
export type ActionContext = {
    requireClient: () => Client;
    state: EngineState;
    set: SetStoreFunction<EngineState>;
    workspaces: () => WorkspaceIndex;
    notice: (input: NoticeInput) => void;
};

export function errorMessage(cause: unknown) {
    if (cause instanceof EngineError) return cause.message;
    if (cause instanceof Error) return cause.message;
    return String(cause);
}
