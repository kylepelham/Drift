import { questionForCard } from "../src/engine/questions";
import { createEngineState } from "../src/engine/store";
import { createActions } from "../src/engine/actions";
import { reduce } from "../src/engine/events";
import { expect, test } from "bun:test";

import type { components } from "../src/engine/native/types";
import type { Client } from "../src/engine/native/client";

const request: components["schemas"]["PermissionRequest"] = {
    id: "permission",
    sessionId: "session",
    messageId: "message",
    callId: "call",
    tool: "edit",
    kind: "edit",
    pattern: "src/storage.ts",
    title: "Update storage handling",
    createdAt: 10,
    diff: "@@ -1 +1 @@\n-before\n+after",
    reason: "outside",
    always: [{ grant: "exact", kind: "edit", target: "src/storage.ts" }],
};

test("permission events retain native grants, reason and proposed diff without synthetic metadata", () => {
    const [state, set] = createEngineState();

    reduce(set, { type: "permission.asked", request }, "C:/repo");

    expect(state.permissions.session[0]).toEqual({ ...request, directory: "" });
    expect("metadata" in state.permissions.session[0]).toBeFalse();
});

test("permission snapshots attach the session's directory without renaming request fields", async () => {
    const [state, set] = createEngineState();
    set("sessions", "session", { directory: "C:/repo" } as never);
    const client = { permissions: async () => [request], questions: async () => [] } as unknown as Client;
    const actions = createActions(
        () => client,
        state,
        set,
        () => ({ path: () => undefined, id: () => undefined }),
    );

    await actions.refreshPermissions();

    expect(state.permissions.session[0]).toEqual({ ...request, directory: "C:/repo" });
});

test("question cards keep native ownership and fill only their presentation defaults", () => {
    const question: components["schemas"]["QuestionRequest"] = {
        id: "question",
        sessionId: "session",
        messageId: "message",
        callId: "call",
        createdAt: 10,
        questions: [{ question: "Which release should receive this change?", options: [{ label: "Next release" }] }],
    };
    const shown = questionForCard(question);

    expect(shown).toEqual({
        ...question,
        async: false,
        questions: [
            {
                question: "Which release should receive this change?",
                header: "",
                options: [{ label: "Next release", description: "" }],
                multiple: false,
                custom: true,
            },
        ],
    });
    expect(question.questions[0].header).toBeUndefined();
});

test("todo events keep native tasks without adding unused IDs or priorities", () => {
    const [state, set] = createEngineState();
    const todos: components["schemas"]["Todo"][] = [{ content: "Review storage migration", status: "in_progress" }];

    reduce(set, { type: "todo.updated", sessionId: "session", todos });

    expect(state.todos.session).toEqual(todos);
});
