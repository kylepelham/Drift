import { toolDisplay } from "../src/ui/tool-presentation";
import { createEngineState } from "../src/engine/store";
import { expect, test } from "bun:test";
import "./source";

import type { ToolPart } from "../src/engine/parts";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined },
    });

test("parent delegated status follows ordinary child errors, resumed work, and completion", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    const part = {
        id: "launch",
        type: "tool_call",
        name: "task",
        sessionId: "parent",
        status: "done",
        input: {},
        output: '<task id="child" state="running">',
    } as never;
    set("errors", "child", "usage limit reached");
    expect(delegatedTaskStatus(state, part, "child")).toBe("error");
    set("errors", "child", undefined!);
    set("status", "child", { type: "busy" });
    expect(delegatedTaskStatus(state, part, "child")).toBe("running");
    set("status", "child", { type: "idle" });
    set("transcripts", "parent", [
        {
            info: { id: "launch-message", sessionID: "parent", role: "assistant", time: { created: 0 } },
            parts: [part],
        },
        {
            info: { id: "done", sessionID: "parent", role: "assistant", time: { created: 1 } },
            parts: [
                {
                    id: "result",
                    type: "text",
                    text: '<task id="child" state="completed">',
                    sessionID: "parent",
                    messageID: "done",
                },
            ],
        },
    ] as never);
    expect(delegatedTaskStatus(state, part, "child")).toBe("completed");
});

test("a live delegated part overrides an older completion marker", async () => {
    const { delegatedTaskClickPolicy, delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    set("transcripts", "parent", [
        {
            info: { id: "old", sessionID: "parent", role: "assistant", time: { created: 1 } },
            parts: [
                {
                    id: "result",
                    type: "text",
                    text: '<task id="child" state="completed">',
                    sessionID: "parent",
                    messageID: "old",
                },
            ],
        },
    ] as never);
    const live = { sessionId: "parent", name: "task", status: "running", input: {}, startedAt: 1 } as never;
    const status = delegatedTaskStatus(state, live, "child");
    expect(status).toBe("running");
    expect(delegatedTaskClickPolicy(status, "child")).toBe("navigate");
});

function taskPart(id: string, output: string): ToolPart {
    return {
        id,
        type: "tool_call",
        name: "task",
        sessionId: "parent",
        messageId: "message",
        callId: id,
        status: "done",
        input: { task_id: "child", description: id, subagent_type: "general" },
        output,
        title: id,
        metadata: { sessionId: "child" },
        startedAt: 1,
        finishedAt: 2,
    };
}

test("finished task cards do not follow a resumed child session's busy, retry, or error state", async () => {
    const { delegatedTaskStatus, delegatedTaskClickPolicy } = await import("../src/ui/tool-delegation");
    const { toolElapsedMs } = await import("../src/ui/tool-duration");
    const [state, set] = createEngineState();
    const original = taskPart(
        "original",
        '<task id="child" state="completed">\n<task_result>First result</task_result>\n</task>',
    );
    const resumed: ToolPart = {
        ...taskPart("resumed", ""),
        status: "running",
        input: { task_id: "child" },
        startedAt: 3,
        finishedAt: undefined,
    };
    for (const type of ["busy", "retry", "idle"] as const) {
        set("status", "child", type === "retry" ? { type, attempt: 1, message: "retry", next: 10 } : { type });
        const previous = delegatedTaskStatus(state, original, "child");
        const current = delegatedTaskStatus(state, resumed, "child");
        expect(previous).toBe("completed");
        expect(current).toBe("running");
        expect(delegatedTaskClickPolicy(previous, "child")).toBe("expand");
        expect(delegatedTaskClickPolicy(current, "child")).toBe("navigate");
        expect(toolElapsedMs(toolDisplay(original), 100)).toBe(1);
        expect(toolElapsedMs(toolDisplay(resumed), 100)).toBe(97);
    }
    set("errors", "child", "The resumed invocation failed");
    expect(delegatedTaskStatus(state, original, "child")).toBe("completed");
    expect(delegatedTaskStatus(state, resumed, "child")).toBe("error");
});

test("failed task cards stay failed when the child is resumed or later completes", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    const failed: ToolPart = {
        ...taskPart("failed", ""),
        status: "error",
        input: {},
        output: "Original failure",
    };
    set("status", "child", { type: "busy" });
    expect(delegatedTaskStatus(state, failed, "child")).toBe("error");
    expect(delegatedTaskStatus(state, taskPart("reported-error", '<task id="child" state="error">'), "child")).toBe(
        "error",
    );
    set("status", "child", { type: "idle" });
    expect(delegatedTaskStatus(state, failed, "child")).toBe("error");
});

test("legacy foreground results stay completed without a loaded parent transcript", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    set("status", "child", { type: "busy" });
    const legacy = taskPart("legacy", "task_id: child\n<task_result>Already finished</task_result>");
    expect(delegatedTaskStatus(state, legacy, "child")).toBe("completed");
});

test("background tasks track work while spawned-thread receipts finish at admission", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    const background = taskPart("background", "Background task started");
    background.metadata = { ...background.metadata, background: true };
    const spawned = {
        ...taskPart("spawned", 'Spawned thread "Child" (id child); its seed prompt was accepted for processing.'),
        name: "spawn_thread",
    };
    for (const type of ["busy", "idle"] as const) {
        set("status", "child", { type });
        expect(delegatedTaskStatus(state, background, "child")).toBe("running");
        expect(delegatedTaskStatus(state, spawned, "child")).toBe("completed");
    }
    set("status", "child", { type: "retry", attempt: 1, message: "retry", next: 10 });
    expect(delegatedTaskStatus(state, spawned, "child")).toBe("completed");
    set("errors", "child", "Sibling failed later");
    expect(delegatedTaskStatus(state, spawned, "child")).toBe("completed");
});

test("spawned-thread rows only track their own pending, running, or failed invocation", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    set("errors", "child", "Unrelated sibling error");
    const spawned = { ...taskPart("spawned", ""), name: "spawn_thread" };
    for (const status of ["pending", "running"] as const) {
        expect(delegatedTaskStatus(state, { ...spawned, status, input: {} }, "child")).toBe("running");
    }
    expect(
        delegatedTaskStatus(
            state,
            {
                ...spawned,
                status: "error",
                input: {},
                output: "Spawn failed",
            },
            "child",
        ),
    ).toBe("error");
});

test("only subagent tasks render child activity progress", async () => {
    const source = await Bun.file(new URL("../src/ui/tool-view.tsx", import.meta.url)).text();
    expect(source).toContainCode('const progress = () => { if (props.part.name !== "task") return null;');
});

test("background completions belong to the invocation preceding them, including after reload", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const original = taskPart("original", '<task id="child" state="running">');
    const resumed = taskPart("resumed", '<task id="child" state="running">');
    const notification = (id: string, status: string) => ({
        id,
        type: "text" as const,
        sessionID: "parent",
        messageID: "message",
        synthetic: true,
        text: `<task id="child" state="${status}">`,
    });
    const parts = [original, notification("first-result", "completed"), resumed];
    // Fresh stores exercise persisted history, not a component-local cache of the old result.
    for (const childStatus of ["busy", "retry", "idle"] as const) {
        const [state, set] = createEngineState();
        set("transcripts", "parent", [
            {
                info: { id: "message", sessionID: "parent", role: "assistant", time: { created: 1 } },
                parts,
            },
        ] as never);
        set(
            "status",
            "child",
            childStatus === "retry"
                ? { type: childStatus, attempt: 1, message: "retry", next: 10 }
                : { type: childStatus },
        );
        expect(delegatedTaskStatus(state, original, "child")).toBe("completed");
        expect(delegatedTaskStatus(state, resumed, "child")).toBe("running");
        set("transcripts", "parent", 0, "parts", [...parts, notification("second-result", "error")]);
        expect(delegatedTaskStatus(state, original, "child")).toBe("completed");
        expect(delegatedTaskStatus(state, resumed, "child")).toBe("error");
    }
});

test("a result from an earlier invocation cannot settle a detached background card", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    set("transcripts", "parent", [
        {
            info: { id: "old", sessionID: "parent", role: "assistant", time: { created: 1 } },
            parts: [taskPart("old", '<task id="child" state="completed">')],
        },
    ] as never);
    expect(delegatedTaskStatus(state, taskPart("new", '<task id="child" state="running">'), "child")).toBe("running");
});

test("background cards ignore another invocation's tool output while awaiting their own notification", async () => {
    const { delegatedTaskStatus } = await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    const background = taskPart("background", '<task id="child" state="running">');
    const later = taskPart("later", '<task id="child" state="completed">');
    set("transcripts", "parent", [
        {
            info: { id: "message", sessionID: "parent", role: "assistant", time: { created: 1 } },
            parts: [background, later],
        },
    ] as never);
    expect(delegatedTaskStatus(state, background, "child")).toBe("running");
    expect(delegatedTaskStatus(state, later, "child")).toBe("completed");
    set("transcripts", "parent", 0, "parts", [
        background,
        later,
        {
            id: "notification",
            type: "text",
            sessionID: "parent",
            messageID: "message",
            synthetic: true,
            text: '<task id="child" state="error">',
        },
    ]);
    expect(delegatedTaskStatus(state, background, "child")).toBe("error");
    expect(delegatedTaskStatus(state, later, "child")).toBe("completed");
});

test("a running delegated row recovers its child when parallel task metadata is missing", async () => {
    const { delegatedChildId, delegatedTaskClickPolicy, delegatedTaskStatus } =
        await import("../src/ui/tool-delegation");
    const [state, set] = createEngineState();
    set("sessions", "child", {
        id: "child",
        parentId: "parent",
        visibility: "hidden",
        title: "Explore service sinks (@explore subagent)",
        createdAt: 1,
        updatedAt: 1,
    } as never);
    const part = {
        name: "task",
        sessionId: "parent",
        status: "running",
        input: { description: "Explore service sinks", subagent_type: "explore" },
        startedAt: 2,
    } as never;
    const childId = delegatedChildId(state, part);
    const status = delegatedTaskStatus(state, part, childId!);
    expect(childId).toBe("child");
    expect(status).toBe("running");
    expect(delegatedTaskClickPolicy(status, childId)).toBe("navigate");

    set("sessions", "duplicate", {
        id: "duplicate",
        parentId: "parent",
        visibility: "hidden",
        title: "Explore service sinks (@explore subagent)",
        createdAt: 2,
        updatedAt: 2,
    } as never);
    expect(delegatedChildId(state, part)).toBeNull();
});

test("running delegated rows navigate while terminal rows expand without lifecycle badges", async () => {
    const { delegatedTaskClickPolicy } = await import("../src/ui/tool-delegation");
    expect(delegatedTaskClickPolicy("running", "child")).toBe("navigate");
    expect(delegatedTaskClickPolicy("completed", "child")).toBe("expand");
    expect(delegatedTaskClickPolicy("error", "child")).toBe("expand");
    const source = await Bun.file("src/ui/tool-view.tsx").text();
    expect(source).toContain("selectSession(spawnedId()!)");
    expect(source).toContain("function spawnedId()");
    expect(source).not.toContain("const spawnedId =");
});
