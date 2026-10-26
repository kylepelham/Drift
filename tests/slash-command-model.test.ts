import { applyMirroredSession, selectedSession } from "../src/state/selection";
import { afterEach, expect, mock, test } from "bun:test";
import { runSlash, slashItem } from "../src/ui/slash";
import { updatePrefs } from "../src/state/prefs";

import type { Engine } from "../src/engine";

const previous = selectedSession();
afterEach(() => {
    applyMirroredSession(previous);
    updatePrefs(null, { model: null });
});

test("a workspace command typed in a chat that has sent nothing runs on the model the composer shows", async () => {
    applyMirroredSession(null);
    updatePrefs(null, { model: { providerID: "openai", modelID: "gpt-6-sol" } });
    const calls: unknown[][] = [];
    const engine = {
        state: {
            commands: [{ name: "audit", description: "Review the codebase" }],
            connection: "online",
            connected: ["openai"],
            providers: [{ id: "openai", name: "OpenAI", models: { "gpt-6-sol": {} } }],
            providerAccounts: {},
            sessions: {},
        },
        actions: {
            newSession: mock(async () => ({ id: "fresh" })),
            useModel: mock(async (...args: unknown[]) => void calls.push(["useModel", ...args])),
            runCommand: mock(async (...args: unknown[]) => void calls.push(["runCommand", ...args])),
        },
    } as unknown as Engine;

    await runSlash(engine, slashItem(engine, "audit")!, "high");

    expect(calls).toEqual([
        ["useModel", "fresh", { providerID: "openai", modelID: "gpt-6-sol" }],
        ["runCommand", "fresh", "audit", "high"],
    ]);
    expect(selectedSession()).toBe("fresh");
});
