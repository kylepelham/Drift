import { applyMirroredSession, selectedSession } from "../src/state/selection";
import { runSlash, slashItem, slashItems } from "../src/ui/slash";
import { afterEach, expect, mock, test } from "bun:test";
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

test("/subagent hands what follows it to the engine, seeding the model of a chat that has sent nothing", async () => {
    applyMirroredSession("chat");
    updatePrefs("chat", { model: { providerID: "openai", modelID: "gpt-6-sol" } });
    const calls: unknown[][] = [];
    const engine = {
        state: {
            commands: [],
            connection: "online",
            connected: ["openai"],
            providers: [{ id: "openai", name: "OpenAI", models: { "gpt-6-sol": {} } }],
            providerAccounts: {},
            sessions: { chat: { id: "chat" } },
        },
        actions: {
            useModel: mock(async (...args: unknown[]) => void calls.push(["useModel", ...args])),
            startSubagent: mock(async (...args: unknown[]) => void calls.push(["startSubagent", ...args])),
        },
    } as unknown as Engine;

    const item = slashItem(engine, "subagent")!;
    expect(item.usage).toBe("[seq|async] [model] <prompt>");
    await runSlash(engine, item, "seq look at the parser");

    expect(calls).toEqual([
        ["useModel", "chat", { providerID: "openai", modelID: "gpt-6-sol" }],
        ["startSubagent", "chat", "seq look at the parser"],
    ]);
    updatePrefs("chat", { model: null });
});

test("/subagent is offered only in a conversation the engine still runs", () => {
    const offered = (sessions: Record<string, unknown>) =>
        slashItems({ state: { commands: [], sessions } } as unknown as Engine, "sub").map((item) => item.name);
    applyMirroredSession("chat");

    expect(offered({ chat: { id: "chat", visibility: "visible" } })).toEqual(["subagent"]);
    expect(offered({ chat: { id: "chat", visibility: "hidden", parentId: "parent" } })).toEqual([]);
    expect(offered({ chat: { id: "chat", visibility: "visible", archivedAt: 1 } })).toEqual([]);
});
