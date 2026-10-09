import { childrenOf, createEngineState, putSession, savedChoice, sessionsFor } from "../src/engine/store";
import { hiddenParent, sessionInWorkspace } from "../src/engine/sessions";
import { applySessionSnapshot, reduce } from "../src/engine/events";
import { expect, test } from "bun:test";

import type { components } from "../src/engine/native/types";

const workspaces = { path: () => "C:/repo", id: () => "workspace" };
const session: components["schemas"]["Session"] = {
    id: "parent",
    workspaceId: "workspace",
    visibility: "sibling",
    title: "Review the storage changes",
    agent: "build",
    createdAt: 10,
    updatedAt: 20,
};

test("native session fields drive thread order, worker nesting and sibling links", () => {
    const [state, set] = createEngineState();
    const worker = { ...session, id: "worker", parentId: "parent", visibility: "hidden" as const, createdAt: 30 };
    const sibling = { ...session, id: "sibling", parentId: "parent", updatedAt: 40 };

    for (const current of [session, worker, sibling]) {
        reduce(set, { type: "session.created", session: current }, undefined, undefined, workspaces);
    }

    expect(sessionsFor(state, "c:\\repo\\").map((current) => current.id)).toEqual(["sibling", "parent"]);
    expect(childrenOf(state, "parent").map((current) => current.id)).toEqual(["worker"]);
    expect(state.links.sibling).toBe("parent");
    expect(hiddenParent(state.sessions.worker)).toBe("parent");
});

test("a session choice reads native model, agent and reasoning fields", () => {
    const [state, set] = createEngineState();
    const chosen = { ...session, agent: "plan", variant: "high", model: { provider: "openai", model: "gpt-6-astra" } };

    putSession(set, sessionInWorkspace(chosen, workspaces));

    expect(savedChoice(state, "parent")).toEqual({
        agent: "plan",
        variant: "high",
        model: { providerID: "openai", modelID: "gpt-6-astra" },
    });
    expect(state.sessionModels.parent).toEqual({ providerID: "openai", modelID: "gpt-6-astra" });
});

test("a scoped session snapshot keeps native archived rows and clears dropped revert markers", () => {
    const [state, set] = createEngineState();
    const archived = { ...session, id: "archived", archivedAt: 50 };
    const reverted = { ...session, revert: { messageId: "prompt" } };

    putSession(set, sessionInWorkspace(archived, workspaces));
    putSession(set, sessionInWorkspace(reverted, workspaces));
    const captured = { ...state.revisions };
    applySessionSnapshot(set, {
        sessions: [sessionInWorkspace(session, workspaces)],
        captured,
        scope: { directory: "C:/repo" },
    });

    expect(state.sessions.archived.archivedAt).toBe(50);
    expect(state.sessions.parent.revert).toBeUndefined();
});
