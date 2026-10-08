import { sessionNeedsAttention, sidebarWorkers } from "../src/state/permission-attention";
import { createEngineState } from "../src/engine/store";
import { expect, test } from "bun:test";

test("every ask that arrives waits on the user: the engine already answered what auto-accept and always cover", () => {
    const [state, set] = createEngineState();
    expect(sessionNeedsAttention(state, "s1")).toBeFalse();
    set("permissions", "s1", [
        {
            id: "p1",
            sessionId: "s1",
            kind: "bash",
            tool: "bash",
            messageId: "m1",
            callId: "c1",
            title: "bash",
            pattern: "git status",
            createdAt: 1,
        },
    ]);
    expect(sessionNeedsAttention(state, "s1")).toBeTrue();
    set("permissions", "s1", []);
    set("questions", "s1", [{ id: "q1", sessionId: "s1", questions: [] }] as never);
    expect(sessionNeedsAttention(state, "s1")).toBeTrue();
});

test("the webview no longer answers asks or keeps an always rule of its own", async () => {
    const composer = await Bun.file("src/ui/composer.tsx").text();
    expect(composer).not.toContain("replyPermission(");
    expect(await Bun.file("src/state/permission-attention.ts").text()).not.toContain("metadata.always");
});

test("the sidebar shows a subagent while it runs, waits on the user, or is open", () => {
    const [state, set] = createEngineState();
    for (const id of ["running", "asking", "done"])
        set("sessions", id, { id, parentId: "parent", visibility: "hidden", createdAt: 1, updatedAt: 1 } as never);
    set("status", "running", { type: "busy" });
    set("status", "done", { type: "idle" });
    set("questions", "asking", [{ id: "q1", sessionId: "asking", questions: [] }] as never);
    expect(
        sidebarWorkers(state, "parent")
            .map((s) => s.id)
            .sort(),
    ).toEqual(["asking", "running"]);
    set("status", "running", { type: "idle" });
    set("questions", "asking", []);
    expect(sidebarWorkers(state, "parent")).toEqual([]);
    expect(sidebarWorkers(state, "parent", "done").map((s) => s.id)).toEqual(["done"]);
});

test("the sidebar shows a subagent resumed in the background while it waits for a slot", () => {
    const [state, set] = createEngineState();
    set("sessions", "worker", {
        id: "worker",
        parentId: "parent",
        visibility: "hidden",
        createdAt: 1,
        updatedAt: 1,
    } as never);
    set("status", "worker", { type: "idle" });
    set("tasks", "parent", [{ id: "t1", sessionId: "worker", state: "queued", mode: "background" }] as never);
    expect(sidebarWorkers(state, "parent").map((s) => s.id)).toEqual(["worker"]);

    set("tasks", "parent", [{ id: "t1", sessionId: "worker", state: "replied", mode: "background" }] as never);
    expect(sidebarWorkers(state, "parent")).toEqual([]);
});
