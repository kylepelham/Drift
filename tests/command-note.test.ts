import { commandNote } from "../src/ui/clarification-answer";
import { expect, test } from "bun:test";

import type { MessageEntry } from "../src/engine/store";

function prompt(...parts: Record<string, unknown>[]): MessageEntry {
    return {
        info: { id: "msg_1", sessionId: "ses_1", role: "user", createdAt: 0 },
        parts: parts.map((part, at) => ({ id: `prt_${at}`, messageId: "msg_1", sessionId: "ses_1", ...part })),
    } as never;
}

const known = (name: string) => ["audit", "review"].includes(name);

test("a recorded skill command shows as its name and arguments, with nothing more to open", () => {
    const note = commandNote(
        prompt({ type: "text", text: "/audit high src" }, { type: "command", name: "audit", arguments: "high src" }),
        () => false,
    );

    expect(note).toMatchObject({ command: "/audit", preview: "high src", text: "/audit high src" });
    expect(note?.body).toBeUndefined();
});

test("a command whose template expanded opens to what it sent", () => {
    const note = commandNote(
        prompt(
            { type: "text", text: "Run tests for the parser" },
            { type: "command", name: "test", arguments: "the parser" },
        ),
        known,
    );

    expect(note).toMatchObject({ command: "/test", text: "/test the parser", body: "Run tests for the parser" });
});

test("before commands were recorded, a bare known command still shows as one", () => {
    expect(commandNote(prompt({ type: "text", text: "/review high" }), known)).toMatchObject({
        command: "/review",
        preview: "high",
    });
    expect(commandNote(prompt({ type: "text", text: "/review" }), known)?.preview).toBe("");
});

test("prose, unknown names and multi-line prompts stay ordinary messages", () => {
    expect(commandNote(prompt({ type: "text", text: "please /review this" }), known)).toBeUndefined();
    expect(commandNote(prompt({ type: "text", text: "/usr/bin is missing" }), known)).toBeUndefined();
    expect(commandNote(prompt({ type: "text", text: "/review high\nand also fix it" }), known)).toBeUndefined();
});
