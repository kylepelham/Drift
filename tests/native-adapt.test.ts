import { hiddenParent, sessionInWorkspace } from "../src/engine/sessions";
import { adaptMessage, adaptPart } from "../src/engine/native/adapt";
import { toolElapsedMs } from "../src/ui/tool-duration";
import { createEngineState } from "../src/engine/store";
import { variantNames } from "../src/engine/catalog";
import { reduce } from "../src/engine/events";
import { expect, test } from "bun:test";

import type { components } from "../src/engine/native/types";

const workspaces = { path: (id: string) => (id === "w1" ? "C:/repo" : undefined), id: () => "w1" };

const session: components["schemas"]["Session"] = {
    id: "ses_1",
    workspaceId: "w1",
    visibility: "sibling",
    title: "Fix bug",
    agent: "build",
    model: { provider: "anthropic", model: "claude" },
    createdAt: 10,
    updatedAt: 20,
};

test("sessions map workspace ids to directories and keep archive time", () => {
    const shown = sessionInWorkspace({ ...session, archivedAt: 30 }, workspaces);

    expect(shown.directory).toBe("C:/repo");
    expect(shown).toMatchObject({ workspaceId: "w1", createdAt: 10, updatedAt: 20, archivedAt: 30 });
    expect(shown.model).toEqual({ provider: "anthropic", model: "claude" });
    expect(sessionInWorkspace({ ...session, workspaceId: "missing" }, workspaces).directory).toBe("missing");
});

test("a model's reasoning levels from the catalog become the picker's variants, in order", () => {
    const model = {
        id: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        reasoning: true,
        variants: [
            { name: "low", kind: "effort", level: "low" },
            { name: "max", kind: "effort", level: "max" },
        ],
    };
    const plain = { id: "claude-haiku", name: "Claude Haiku" };
    expect(variantNames(model)).toEqual(["low", "max"]);
    expect(variantNames(plain)).toEqual([]);
});

test("subagents nest under their parent while spawned threads stay top level with a link", () => {
    const subagent = sessionInWorkspace(
        { ...session, id: "ses_2", parentId: "ses_1", visibility: "hidden" },
        workspaces,
    );
    const spawned = sessionInWorkspace({ ...session, id: "ses_3", parentId: "ses_1" }, workspaces);

    expect(hiddenParent(subagent)).toBe("ses_1");
    expect(hiddenParent(spawned)).toBeUndefined();
    expect(spawned.parentId).toBe("ses_1");
});

test("assistant messages carry tokens, cost and errors in the legacy shape", () => {
    const info = adaptMessage(
        {
            id: "msg_1",
            sessionId: "ses_1",
            role: "assistant",
            status: "error",
            model: { provider: "anthropic", model: "claude" },
            usage: { input: 5, output: 7, cacheRead: 1, cacheWrite: 2 },
            cost: 0.5,
            error: "boom",
            createdAt: 1,
            finishedAt: 2,
        },
        "C:/repo",
    );
    if (info.role !== "assistant") throw new Error("expected assistant");
    expect(info.tokens).toEqual({ input: 5, output: 7, reasoning: 0, cache: { read: 1, write: 2 } });
    expect(info.cost).toBe(0.5);
    expect(info.error).toEqual({ name: "UnknownError", data: { message: "boom" } });
    expect(info.time).toEqual({ created: 1, completed: 2 });
});

test("sessions and messages keep the agent they actually ran as", () => {
    expect(sessionInWorkspace({ ...session, agent: "plan" }, workspaces).agent).toBe("plan");
    expect(sessionInWorkspace({ ...session, variant: "max" }, workspaces).variant).toBe("max");
    const base = {
        sessionId: "ses_1",
        model: { provider: "anthropic", model: "claude" },
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        cost: 0,
        createdAt: 1,
        agent: "plan",
    };
    const asked = adaptMessage({ ...base, id: "msg_u", role: "user", status: "done" }, "C:/repo");
    const replied = adaptMessage({ ...base, id: "msg_a", role: "assistant", status: "done" }, "C:/repo");
    expect(asked.role === "user" && asked.agent).toBe("plan");
    expect(replied.role === "assistant" && replied.mode).toBe("plan");
});

test("a reply that stopped at its output limit shows why", () => {
    const base = {
        id: "msg_3",
        sessionId: "ses_1",
        role: "assistant" as const,
        model: { provider: "anthropic", model: "claude" },
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        cost: 0,
        createdAt: 1,
    };
    type Shown = { finish?: string; error?: { name: string; data: { message: string } } };
    const cut = adaptMessage(
        { ...base, status: "done", ending: "length", error: "The reply stopped at the output limit (32000 tokens)." },
        "C:/repo",
    ) as Shown;
    expect(cut.finish).toBe("length");
    expect(cut.error).toEqual({
        name: "MessageOutputLengthError",
        data: { message: "The reply stopped at the output limit (32000 tokens)." },
    });
    const refused = adaptMessage(
        { ...base, status: "done", ending: "refused", error: "Blocked, in any words." },
        "C:/repo",
    ) as Shown;
    expect(refused.finish).toBe("content-filter");
    expect(refused.error?.data.message).toBe("Blocked, in any words.");
    const paused = adaptMessage(
        { ...base, status: "paused", error: "Paused after 200 steps, this turn's limit." },
        "C:/repo",
    ) as { error?: { name: string; data: { message: string } } };
    expect(paused.error).toEqual({
        name: "MessageAbortedError",
        data: { message: "Paused after 200 steps, this turn's limit." },
    });
    const whole = adaptMessage({ ...base, status: "done" }, "C:/repo") as { finish?: string; error?: unknown };
    expect(whole.finish).toBe("stop");
    expect(whole.error).toBeUndefined();
});

test("a mention keeps the file it names, so undo restores it as a mention and its chip can open it", async () => {
    const { draftFromMessage } = await import("../src/state/composer");
    const row = {
        id: "prt_m",
        messageId: "msg_m",
        sessionId: "ses_1",
        type: "file" as const,
        mime: "text/plain",
        name: "src/db.ts",
        url: "data:text/plain;base64,eA==",
        path: "src/db.ts",
    };
    const mention = adaptPart(row);
    expect(mention).toMatchObject({ type: "file", source: { type: "file", path: "src/db.ts" } });
    const pasted = adaptPart({ ...row, id: "prt_p", name: "notes.txt", path: undefined });
    expect("source" in pasted).toBe(false);
    const entry = {
        info: { id: "msg_m", sessionID: "ses_1", role: "user", time: { created: 1 } },
        parts: [{ id: "t", type: "text", text: "check @src/db.ts" }, mention],
    } as never;
    expect(draftFromMessage(entry).mentions).toEqual(["src/db.ts"]);
});

test("one reference is one mention: a path never matches inside a longer one", async () => {
    const { mentionFiles } = await import("../src/ui/composer-mentions");
    const text = "what is in @src/database/database.ts, and @README.md.";
    const sent = mentionFiles(text, ["src/database", "src/database/database.ts", "README.md"], "C:/repo");
    expect(sent.map((file) => file.filename)).toEqual(["database.ts", "README.md"]);
});

test("a delivered background result is engine text, not the user's words", () => {
    const part = adaptPart({
        id: "prt_9",
        messageId: "msg_9",
        sessionId: "ses_1",
        type: "task_result",
        taskId: "task_1",
        workerSessionId: "ses_w",
        description: "Survey",
        outcome: "replied",
        text: "three things",
    });
    expect(part).toMatchObject({
        type: "text",
        synthetic: true,
        sessionID: "ses_1",
        text: 'Background task "Survey" replied:\n\nthree things',
    });
});

test("a part the engine could not read is not drawn or reused, and keeps its stored text", async () => {
    const { partVisible } = await import("../src/ui/parts");
    const raw = '{"type":"snapshot","snapshot":"abc"}';
    const part = adaptPart({ id: "prt_u", messageId: "msg_u", sessionId: "ses_1", type: "unknown", raw });
    expect(part).toMatchObject({
        type: "text",
        text: "",
        synthetic: true,
        ignored: true,
        metadata: { driftUnknownPart: raw },
    });
    expect(partVisible(part)).toBeFalse();
});

test("an async answer becomes the Answered row the transcript already draws", async () => {
    const { clarificationAnswer } = await import("../src/ui/clarification-answer");
    const part = adaptPart({
        id: "prt_a",
        messageId: "msg_a",
        sessionId: "ses_1",
        type: "clarification",
        requestId: "q_1",
        items: [{ header: "Deploy", question: "Deploy?", answers: ["yes"] }],
    });
    expect(part).toMatchObject({
        type: "text",
        text: "Deploy?\nAnswer: yes",
        metadata: { driftClarification: { version: 1, requestID: "q_1" } },
    });
    const entry = {
        info: {
            id: "msg_a",
            sessionID: "ses_1",
            role: "user",
            time: { created: 1 },
            agent: "build",
            model: { providerID: "", modelID: "" },
        },
        parts: [part],
    } as never;
    expect(clarificationAnswer(entry)).toEqual({
        items: [{ header: "Deploy", question: "Deploy?", answers: ["yes"] }],
        text: "Deploy?\nyes",
        preview: "yes",
    });
});

test("an async question keeps its flag for the Answer later card", async () => {
    const { questionForCard } = await import("../src/engine/questions");
    const request = {
        id: "q_1",
        sessionId: "ses_1",
        messageId: "msg_1",
        callId: "call_1",
        createdAt: 1,
        async: true,
        questions: [{ question: "Deploy?", header: "Deploy", options: [] }],
    };
    expect(questionForCard(request).async).toBe(true);
    expect(questionForCard({ ...request, async: false }).async).toBe(false);
});

test("a compaction becomes the boundary part and summary message the transcript already draws", () => {
    const summary = adaptMessage(
        {
            id: "msg_2",
            sessionId: "ses_1",
            role: "assistant",
            status: "done",
            model: { provider: "anthropic", model: "claude" },
            usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            cost: 0,
            createdAt: 1,
            summary: true,
        },
        "C:/repo",
    );
    expect((summary as { summary?: boolean }).summary).toBeTrue();
    const boundary = adaptPart({
        id: "prt_1",
        messageId: "msg_1",
        sessionId: "ses_1",
        type: "compaction",
        auto: true,
        tailFrom: "msg_0",
    });
    expect(boundary).toEqual({ id: "prt_1", sessionID: "ses_1", messageID: "msg_1", type: "compaction", auto: true });
});

test("tool call statuses become legacy tool states", () => {
    const base = {
        id: "prt_1",
        messageId: "msg_1",
        sessionId: "ses_1",
        type: "tool_call" as const,
        callId: "t",
        name: "read",
        input: { path: "a" },
    };
    const done = adaptPart({ ...base, status: "done", output: "1: a", title: "a", startedAt: 1, finishedAt: 2 });
    if (done.type !== "tool") throw new Error("expected tool");
    expect(done.tool).toBe("read");
    expect(done.state).toEqual({
        status: "completed",
        input: { path: "a", filePath: "a" },
        output: "1: a",
        title: "a",
        metadata: {},
        time: { start: 1, end: 2 },
    });
    const denied = adaptPart({ ...base, status: "denied", output: "Permission denied by the user." });
    if (denied.type !== "tool") throw new Error("expected tool");
    expect(denied.state.status).toBe("error");
    const pending = adaptPart({ ...base, status: "pending" });
    if (pending.type !== "tool") throw new Error("expected tool");
    expect(pending.state).toEqual({ status: "pending", input: { path: "a", filePath: "a" }, raw: "" });
});

test("an undo marker and a native removed message reach the store", () => {
    const undone = sessionInWorkspace({ ...session, revert: { messageId: "msg_5", kept: ["a.txt"] } }, workspaces);
    expect(undone.revert).toEqual({ messageId: "msg_5", kept: ["a.txt"] });
    expect(sessionInWorkspace(session, workspaces).revert).toBeUndefined();
    const [state, set] = createEngineState();
    set("transcripts", "ses_1", [{ info: { id: "msg_5", role: "user" }, parts: [] }] as never);

    reduce(set, { type: "message.removed", sessionId: "ses_1", messageId: "msg_5" });
    expect(state.transcripts.ses_1).toEqual([]);
});

test("a retry wait becomes the retry status the notice draws", () => {
    const [state, set] = createEngineState();

    reduce(set, {
        type: "session.retry",
        sessionId: "ses_1",
        attempt: 2,
        message: "overloaded (529): busy",
        nextAt: 5000,
    });
    expect(state.status.ses_1).toEqual({ type: "retry", attempt: 2, message: "overloaded (529): busy", next: 5000 });
});

test("native events update busy status, text and pending permissions", () => {
    const [state, set] = createEngineState();
    reduce(set, { type: "session.status", sessionId: "ses_1", status: "running" });
    expect(state.status.ses_1).toEqual({ type: "busy" });

    set("loaded", "s", true);
    set("transcripts", "s", [{ info: { id: "m", role: "assistant" }, parts: [] }] as never);
    reduce(set, { type: "part.created", part: { type: "text", id: "p", sessionId: "s", messageId: "m", text: "say" } });
    reduce(set, { type: "part.delta", sessionId: "s", messageId: "m", partId: "p", delta: "hi", offset: 3 });
    expect(state.transcripts.s[0].parts[0]).toMatchObject({ type: "text", text: "sayhi" });

    const request: components["schemas"]["PermissionRequest"] = {
        id: "perm_1",
        sessionId: "ses_1",
        messageId: "msg_1",
        callId: "t",
        tool: "bash",
        kind: "bash",
        pattern: "cargo test",
        title: "Run tests",
        createdAt: 5,
    };
    reduce(set, { type: "permission.asked", request });
    expect(state.permissions.ses_1[0].id).toBe("perm_1");
    expect(state.permissions.ses_1[0]).toMatchObject({
        id: "perm_1",
        kind: "bash",
        pattern: "cargo test",
        callId: "t",
        tool: "bash",
    });
    reduce(set, { type: "workspace.created", workspace: { id: "w", path: "p", name: "n", icon: "", lastUsed: 0 } });
    expect(state.permissions.ses_1).toHaveLength(1);
});

test("a call that never started has no duration", () => {
    const row = {
        id: "prt_1",
        messageId: "msg_1",
        sessionId: "ses_1",
        type: "tool_call" as const,
        callId: "c1",
        name: "bash",
        input: { command: "sleep 10" },
        status: "denied" as const,
        output: "denied",
        finishedAt: 1_700_000_000_000,
    };
    const part = adaptPart(row as never);
    expect(part.type).toBe("tool");
    const state = (part as { state: { time?: { start?: number; end?: number } } }).state;
    expect(state.time?.start).toBeUndefined();
    expect(state.time?.end).toBe(1_700_000_000_000);
    expect(toolElapsedMs(state as never, Date.now())).toBeUndefined();
});

test("a delta a snapshot already holds is skipped, and one it lacks is added where it starts", async () => {
    const { withDelta } = await import("../src/engine/events");
    expect(withDelta("hello world", "world", 6), "the snapshot already has it").toBe("hello world");
    expect(withDelta("hello wo", "world", 6), "cut short mid-delta").toBe("hello world");
    expect(withDelta("hello ", "world", 6)).toBe("hello world");
    expect(withDelta("hello ", "world"), "an engine that sends no offset appends").toBe("hello world");
});

test("native file tools reach the UI's rows, file actions and citations under the names they read", async () => {
    const { builtinFileTargets } = await import("../src/tool-actions");
    const { patchFiles, toolInfo } = await import("../src/ui/parts");
    const diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,3 +3,3 @@\n x\n-old\n+new\n y\n";
    const tool = (name: string, input: object, metadata: object) =>
        adaptPart({
            id: `prt_${name}`,
            messageId: "msg_1",
            sessionId: "ses_1",
            type: "tool_call",
            callId: `c_${name}`,
            name,
            input,
            metadata,
            status: "done",
            output: "Edited",
            startedAt: 1,
            finishedAt: 2,
        } as never) as never;
    const edit = tool(
        "edit",
        { path: "src/a.rs", old_string: "old", new_string: "new" },
        {
            files: ["C:/repo/src/a.rs"],
            diff,
            changes: [{ path: "src/a.rs", before: "h1", after: "h2" }],
            fileChanges: [
                {
                    filePath: "C:/repo/src/a.rs",
                    relativePath: "src/a.rs",
                    type: "update",
                    patch: diff,
                    additions: 1,
                    deletions: 1,
                },
            ],
        },
    );
    expect(toolInfo(edit).subtitle).toBe("a.rs");
    expect(builtinFileTargets(edit, "C:/repo")).toEqual([
        { path: "C:/repo/src/a.rs", label: "C:/repo/src/a.rs", line: 4 },
    ]);
    const read = tool("read", { path: "README.md" }, {});
    expect(toolInfo(read).subtitle?.startsWith("README.md")).toBe(true);
    const patch = "*** Begin Patch\n*** Add File: b.txt\n+b\n*** End Patch\n";
    const patched = tool(
        "apply_patch",
        { patch },
        {
            files: ["C:/repo/b.txt"],
            changes: [{ path: "b.txt", before: null, after: "h3" }],
            fileChanges: [
                {
                    filePath: "C:/repo/b.txt",
                    relativePath: "b.txt",
                    type: "add",
                    patch: "@@ -0,0 +1 @@\n+b\n",
                    additions: 1,
                    deletions: 0,
                },
            ],
        },
    );
    expect(
        patchFiles(patched).map((file: { relativePath?: string; additions: number }) => [
            file.relativePath,
            file.additions,
        ]),
    ).toEqual([["b.txt", 1]]);
    expect(toolInfo(patched).subtitle).toBe("b.txt");
    expect(builtinFileTargets(patched, "C:/repo").map((target) => target.path)).toEqual(["C:/repo/b.txt"]);
    expect((patched as { state: { input: { patchText?: string } } }).state.input.patchText).toBe(patch);
    const metadata = (patched as { state: { metadata: { diff?: string; changes?: unknown } } }).state.metadata;
    expect(metadata.diff, "a one-file patch shows its diff").toBe("@@ -0,0 +1 @@\n+b\n");
    expect(metadata.changes, "undo's own record is left as it is").toEqual([
        { path: "b.txt", before: null, after: "h3" },
    ]);
    const mcp = tool("docs_lookup", { path: "/api/users" }, {});
    expect(
        (mcp as { state: { input: Record<string, unknown> } }).state.input,
        "an MCP tool's arguments are shown as it sent them",
    ).toEqual({ path: "/api/users" });
});
