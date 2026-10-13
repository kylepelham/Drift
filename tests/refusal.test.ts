import { refusalTrigger } from "../src/engine/refusal";
import { expect, test } from "bun:test";

import type { MessageEntry } from "../src/engine/store";

let next = 0;
function entry(role: "user" | "assistant", status: string, parts: object[]): MessageEntry {
    const id = `msg_${++next}`;
    const info = { id, sessionId: "s", role, status, usage: {}, cost: 0, createdAt: next, summary: false };
    return { info, parts: parts.map((part, index) => ({ id: `${id}_${index}`, ...part })) } as unknown as MessageEntry;
}
const text = (value: string) => ({ type: "text", text: value });
const call = (name: string, input: object) => ({ type: "tool_call", name, input, status: "done", callId: name });

test("after a reply that called tools, those calls' results are what was new", () => {
    const tools = entry("assistant", "done", [
        text("reading"),
        call("read", { filePath: "a.rs" }),
        call("bash", { command: "ls" }),
    ]);
    const refused = entry("assistant", "done", []);
    const found = refusalTrigger([entry("user", "done", [text("start")]), tools, refused], refused.info.id);

    expect(found?.tools.map((part) => part.name)).toEqual(["read", "bash"]);
    expect(found?.prompts).toEqual([]);
});

test("the user's messages since the last reply are new, attachments included, and failed attempts are skipped", () => {
    const answered = entry("assistant", "done", [text("done")]);
    const first = entry("user", "done", [
        text("first ask"),
        { type: "file", name: "dump.bin", mime: "x", url: "data:" },
    ]);
    const steered = entry("user", "done", [text("and this")]);
    const failed = entry("assistant", "error", []);
    const refused = entry("assistant", "done", []);

    const found = refusalTrigger([answered, first, steered, failed, refused], refused.info.id);

    expect(found).toEqual({ prompts: ["first ask", "and this"], files: ["dump.bin"], tools: [] });
});

test("nothing before the refusal, or an unknown message, finds nothing", () => {
    const refused = entry("assistant", "done", []);
    expect(refusalTrigger([refused], refused.info.id)).toBeUndefined();
    expect(refusalTrigger([refused], "missing")).toBeUndefined();
});
