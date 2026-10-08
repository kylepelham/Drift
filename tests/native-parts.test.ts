import { promptPartText, toolInput, toolMetadata } from "../src/engine/parts";
import { clarificationAnswer } from "../src/ui/clarification-answer";
import { patchFiles, partVisible, toolInfo } from "../src/ui/parts";
import { toolDisplay } from "../src/ui/tool-presentation";
import { draftFromMessage } from "../src/state/composer";
import { builtinFileTargets } from "../src/tool-actions";
import { toolElapsedMs } from "../src/ui/tool-duration";
import { messageText } from "../src/engine/store";
import { expect, test } from "bun:test";

import type { Part, ToolPart } from "../src/engine/parts";
import type { MessageEntry } from "../src/engine/store";

const base = { id: "part", sessionId: "session", messageId: "message" };

function entry(parts: Part[]): MessageEntry {
    return {
        info: {
            id: "message",
            sessionId: "session",
            role: "user",
            status: "done",
            createdAt: 1,
            cost: 0,
            usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        },
        parts,
    };
}

function tool(name: string, input: unknown, metadata: ToolPart["metadata"] = {}): ToolPart {
    return {
        ...base,
        type: "tool_call",
        callId: "call",
        name,
        input,
        metadata,
        status: "done",
        output: "Edited",
        startedAt: 1,
        finishedAt: 2,
    };
}

test("mentions and uploads restore their native names and paths without synthetic file sources", () => {
    const mention: Part = {
        ...base,
        type: "file",
        mime: "text/plain",
        name: "src/storage.ts",
        path: "src/storage.ts",
        url: "data:text/plain;base64,eA==",
    };
    const upload: Part = { ...mention, id: "upload", name: "notes.txt", path: undefined };
    const restored = draftFromMessage(
        entry([{ ...base, id: "text", type: "text", text: "Review @src/storage.ts" }, mention, upload]),
    );

    expect(restored.mentions).toEqual(["src/storage.ts"]);
    expect(restored.staged[0].filename).toBe("notes.txt");
    expect(mention).not.toHaveProperty("source");
});

test("worker results and unknown stored parts are neither drawn nor reused as user prompts", () => {
    const result: Part = {
        ...base,
        type: "task_result",
        taskId: "task",
        workerSessionId: "worker",
        description: "Review storage",
        outcome: "replied",
        text: "The migration is safe.",
    };
    const unknown: Part = { ...base, id: "unknown", type: "unknown", raw: '{"type":"snapshot","snapshot":"saved"}' };

    expect(messageText(entry([result, unknown]))).toBe("");
    expect(partVisible(result)).toBeFalse();
    expect(partVisible(unknown)).toBeFalse();
    expect(unknown.raw).toContain("saved");
});

test("nudges remain engine-written prompt text and clarifications retain their answer boundaries", () => {
    const nudge: Part = { ...base, type: "nudge", text: "Continue toward the goal." };
    const clarified: Part = {
        ...base,
        type: "clarification",
        requestId: "question",
        items: [{ header: "Deploy", question: "Deploy the change?", answers: ["Next release"] }],
    };

    expect(promptPartText(nudge)).toBe("Continue toward the goal.");
    expect(clarificationAnswer(entry([clarified]))).toEqual({
        items: clarified.items,
        text: "Deploy the change?\nNext release",
        preview: "Next release",
    });
    expect(promptPartText(clarified)).toBe("Deploy the change?\nAnswer: Next release");
});

test("native tool status and timing produce the same visible row state without mutating the record", () => {
    const read = tool("read", { path: "README.md" });
    const shown = toolDisplay(read);

    expect(shown).toMatchObject({
        status: "completed",
        input: { path: "README.md", filePath: "README.md" },
        time: { start: 1, end: 2 },
    });
    expect(read.input).toEqual({ path: "README.md" });
    expect(toolDisplay({ ...read, status: "running" }).time.end).toBeUndefined();
    expect(toolDisplay({ ...read, status: "pending" }).status).toBe("pending");
    expect(toolDisplay({ ...read, status: "denied", output: "Permission denied by the user." }).status).toBe("error");
});

test("a call denied before starting has no duration", () => {
    const denied = {
        ...tool("bash", { command: "git status" }),
        status: "denied" as const,
        startedAt: undefined,
        finishedAt: 1_700_000_000_000,
    };
    const shown = toolDisplay(denied);

    expect(shown.time.start).toBeUndefined();
    expect(shown.time.end).toBe(1_700_000_000_000);
    expect(toolElapsedMs(shown, Date.now())).toBeUndefined();
});

test("file changes supply tool subtitles, file actions and single-file diffs while preserving undo metadata", () => {
    const diff = "--- a/src/storage.rs\n+++ b/src/storage.rs\n@@ -3,3 +3,3 @@\n x\n-old\n+new\n y\n";
    const edit = tool("edit", { path: "src/storage.rs" }, { files: ["C:/repo/src/storage.rs"], diff });
    const patch = "*** Begin Patch\n*** Add File: notes.txt\n+Notes\n*** End Patch\n";
    const patched = tool(
        "apply_patch",
        { patch },
        {
            files: ["C:/repo/notes.txt"],
            changes: [{ path: "notes.txt", before: null, after: "saved" }],
            fileChanges: [
                {
                    filePath: "C:/repo/notes.txt",
                    relativePath: "notes.txt",
                    type: "add",
                    patch: "@@ -0,0 +1 @@\n+Notes\n",
                    additions: 1,
                    deletions: 0,
                },
            ],
        },
    );

    expect(toolInfo(edit).subtitle).toBe("storage.rs");
    expect(builtinFileTargets(edit, "C:/repo")).toEqual([
        { path: "C:/repo/src/storage.rs", label: "C:/repo/src/storage.rs", line: 4 },
    ]);
    expect(patchFiles(patched).map((file) => [file.relativePath, file.additions])).toEqual([["notes.txt", 1]]);
    expect(toolInfo(patched).subtitle).toBe("notes.txt");
    expect(builtinFileTargets(patched, "C:/repo").map((target) => target.path)).toEqual(["C:/repo/notes.txt"]);
    expect(toolInput(patched).patchText).toBe(patch);
    expect(toolMetadata(patched).diff).toBe("@@ -0,0 +1 @@\n+Notes\n");
    expect(toolMetadata(patched).changes).toEqual(patched.metadata?.changes);
    expect(patched.metadata?.files).toEqual(["C:/repo/notes.txt"]);
    expect(toolInput(tool("docs_lookup", { path: "/api/users" }))).toEqual({ path: "/api/users" });
});
