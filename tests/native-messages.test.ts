import { messageModel, messageProblem } from "../src/engine/messages";
import { expect, test } from "bun:test";

import type { Message } from "../src/engine/messages";

const message: Message = {
    id: "message",
    sessionId: "session",
    role: "assistant",
    status: "done",
    createdAt: 1,
    cost: 0,
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

test.each([
    [{ status: "error" }, { text: "The turn failed", interrupted: false }],
    [
        { status: "error", error: "" },
        { text: "UnknownError", interrupted: false },
    ],
    [
        { status: "aborted", error: "Cancelled" },
        { text: "Interrupted", interrupted: true },
    ],
    [{ status: "paused" }, { text: "Paused", interrupted: true }],
    [{ ending: "length" }, { text: "The reply stopped at the output limit.", interrupted: false }],
    [{ ending: "refused" }, { text: "The provider's safety filter ended the reply.", interrupted: false }],
    [{ status: "streaming", error: "Not a completed failure" }, undefined],
    [{ role: "user", status: "error" }, undefined],
] as const)("native message presentation derives %j without mutating its record", (fields, expected) => {
    const current = { ...message, ...fields };
    const before = JSON.stringify(current);

    expect(messageProblem(current)).toEqual(expected);
    expect(JSON.stringify(current)).toBe(before);
});

test("provider JSON is unwrapped only for presentation and absent message models remain blank", () => {
    const current = {
        ...message,
        status: "error" as const,
        error: '{"error":{"type":"rate_limit","message":"Retry later"}}',
    };

    expect(messageProblem(current)?.text).toBe("rate_limit: Retry later");
    expect(messageModel(message)).toEqual({ providerID: "", modelID: "" });
    expect(messageModel({ ...message, model: { provider: "openai", model: "gpt-6-astra" } })).toEqual({
        providerID: "openai",
        modelID: "gpt-6-astra",
    });
});
