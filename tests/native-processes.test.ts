import { mergeProcesses, putProcesses } from "../src/engine/processes";
import { createEngineState } from "../src/engine/store";
import { createActions } from "../src/engine/actions";
import { expect, test } from "bun:test";

import type { ProcessRecord } from "../src/engine/processes";
import type { Client } from "../src/engine/native/client";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", { value: { getItem: () => null, setItem: () => undefined } });

function processRecord(id: string, overrides: Partial<ProcessRecord> = {}): ProcessRecord {
    return {
        id,
        sessionId: "session",
        command: "npm run dev",
        description: "dev server",
        state: "running",
        startedAt: 1,
        log: `C:/logs/${id}.log`,
        ...overrides,
    };
}

test("an ended process is never brought back to running by a snapshot that raced its end", () => {
    const ended = processRecord("a", { state: "exited", exitCode: 0, endedAt: 5 });

    const merged = mergeProcesses([ended], [processRecord("a"), processRecord("b", { startedAt: 2 })]);

    expect(merged.map((record) => [record.id, record.state])).toEqual([
        ["a", "exited"],
        ["b", "running"],
    ]);
});

test("the dock lists a conversation's processes while any runs, newest first", async () => {
    const { dockProcesses } = await import("../src/ui/process-row");
    const running = processRecord("b", { startedAt: 2 });
    const ended = processRecord("a", { state: "stopped" });

    expect(dockProcesses([ended, running]).map((record) => record.id)).toEqual(["b", "a"]);
    expect(dockProcesses([ended])).toEqual([]);
    expect(dockProcesses(undefined)).toEqual([]);
});

test("opening a conversation loads its processes, and stopping one records what the engine says", async () => {
    const [state, set] = createEngineState();
    const stopped: string[] = [];
    const client = {
        messages: async () => [],
        todos: async () => [],
        tasks: async () => [],
        processes: async () => [processRecord("a")],
        stopProcess: async (id: string) => {
            stopped.push(id);
            return processRecord(id, { state: "stopped" });
        },
    } as unknown as Client;
    const actions = createActions(
        () => client,
        state,
        set,
        () => ({}),
    );

    await actions.reconcileSession("session");
    expect(state.processes.session?.map((record) => record.state)).toEqual(["running"]);

    await actions.stopProcess("a");
    expect(stopped).toEqual(["a"]);
    expect(state.processes.session?.map((record) => record.state)).toEqual(["stopped"]);

    putProcesses(set, state, "session", [processRecord("a")]);
    expect(state.processes.session?.[0]?.state).toBe("stopped");
});

test("a process's running time reads as a person would say it", async () => {
    const { processSpan } = await import("../src/ui/process-row");

    expect(processSpan(6_400)).toBe("6 s");
    expect(processSpan(125_000)).toBe("2 min 5 s");
    expect(processSpan(3_725_000)).toBe("1 h 2 min");
    expect(processSpan(-5)).toBe("0 s");
});
