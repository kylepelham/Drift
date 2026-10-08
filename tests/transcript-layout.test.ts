import { expect, test } from "bun:test";

import type { MessageEntry } from "../src/engine/store";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined },
    });

const tool = (id: string, messageID: string, name = "read") => ({
    id,
    messageId: messageID,
    sessionId: "s1",
    type: "tool_call",
    name,
    callId: id,
    status: "done",
    input: {},
    output: "",
    title: "",
    metadata: {},
    startedAt: 1,
    finishedAt: 2,
});

const text = (id: string, messageID: string) => ({
    id,
    messageID,
    sessionID: "s1",
    type: "text",
    text: id,
    time: { start: 1, end: 2 },
});

test("question tool names distinguish async input and persisted metadata from blocking questions", async () => {
    const { toolInfo } = await import("../src/ui/tool-labels");
    for (const status of ["pending", "running", "done", "error"]) {
        for (const [input, metadata, title] of [
            [{ async: true }, {}, "Async Question"],
            [{}, { async: true, requestID: "que_saved" }, "Async Question"],
            [{ async: false }, {}, "Question"],
            [{}, { answers: [["Yes"]] }, "Question"],
            [{ async: "true" }, {}, "Question"],
            [{}, {}, "Question"],
        ] as const) {
            const part = tool("q1", "a1", "question");
            const info = toolInfo({
                ...part,
                status,
                input: { ...input, questions: [{ header: "Output format" }] },
                metadata,
            } as Parameters<typeof toolInfo>[0]);
            expect(info).toEqual({ title, subtitle: "Output format" });
        }
    }
});

test("all locales distinguish the async question tool name", async () => {
    for (const locale of [
        "en",
        "ar",
        "br",
        "bs",
        "da",
        "de",
        "es",
        "fr",
        "ja",
        "ko",
        "no",
        "pl",
        "ru",
        "th",
        "tr",
        "uk",
        "zh",
        "zht",
    ]) {
        const { dict, drift } = await import(`../src/i18n/${locale}`);
        expect(drift["drift.tool.asyncQuestion"]).toBeString();
        expect(drift["drift.tool.asyncQuestion"].length).toBeGreaterThan(0);
        expect(drift["drift.tool.asyncQuestion"]).not.toBe(dict["notification.question.title"]);
    }
});

const assistant = (id: string, parts: unknown[], extra: Record<string, unknown> = {}) =>
    ({
        info: { id, sessionId: "s1", role: "assistant", createdAt: 1, ...extra },
        parts,
    }) as MessageEntry;

test("assistant grouping and pitch are invariant to provider message chunking", async () => {
    const { groupAssistantEntries } = await import("../src/ui/message-groups");
    const { timelinePitch } = await import("../src/ui/chat");
    const one = [assistant("a1", [tool("r1", "a1"), tool("r2", "a1"), tool("r3", "a1"), text("answer", "a1")])];
    const split = [
        assistant("a1", [tool("r1", "a1"), tool("r2", "a1")]),
        assistant("a2", [tool("r3", "a2"), text("answer", "a2")]),
    ];
    const structure = (entries: MessageEntry[]) => {
        const grouped = groupAssistantEntries(entries);
        const visible = entries.flatMap((entry) =>
            (grouped.get(entry.info.id) ?? []).map((group) => ({ entry, group })),
        );
        return visible.map(({ entry, group }, index) => ({
            type: "explored" in group ? "context" : group.part.type,
            parts: "explored" in group ? group.explored.map((part) => part.id) : [group.part.id],
            pitch: timelinePitch(entry, visible[index + 1]?.entry),
        }));
    };

    expect(structure(one)).toEqual([
        { type: "context", parts: ["r1", "r2", "r3"], pitch: "part" },
        { type: "text", parts: ["answer"], pitch: "none" },
    ]);
    expect(structure(split)).toEqual(structure(one));
});

test("context grouping stops at meaningful transcript boundaries", async () => {
    const { groupAssistantEntries } = await import("../src/ui/message-groups");
    const first = assistant("a1", [tool("r1", "a1")]);
    const user = {
        info: { id: "u1", sessionID: "s1", role: "user", time: { created: 2 } },
        parts: [text("question", "u1")],
    } as MessageEntry;
    const second = assistant("a2", [tool("r2", "a2")]);
    const grouped = groupAssistantEntries([first, user, second]);

    expect(grouped.get("a1")?.map((group) => ("explored" in group ? group.explored.length : 0))).toEqual([1]);
    expect(grouped.get("a2")?.map((group) => ("explored" in group ? group.explored.length : 0))).toEqual([1]);
});

test("timeline pitch keeps turn, compaction, and error breaks without trailing space", async () => {
    const { timelinePitch } = await import("../src/ui/chat");
    const regular = assistant("a1", [text("one", "a1")]);
    const continuation = assistant("a2", [text("two", "a2")]);
    const summary = assistant("a3", [text("summary", "a3")], { summary: true });
    const failed = assistant("a4", [], { status: "error", error: "The turn failed" });
    const user = {
        info: { id: "u1", sessionID: "s1", role: "user", time: { created: 2 } },
        parts: [text("question", "u1")],
    } as MessageEntry;

    expect(timelinePitch(regular, continuation)).toBe("part");
    expect(timelinePitch(regular, user)).toBe("turn");
    expect(timelinePitch(regular, summary)).toBe("turn");
    expect(timelinePitch(regular, failed)).toBe("turn");
    expect(timelinePitch(regular)).toBe("none");
});

test("native message rates preserve the reasoning fallback when parts carry no generation timestamps", async () => {
    const { generationMs, tokensPerSecond } = await import("../src/ui/message");
    const entry = {
        info: {
            id: "a1",
            sessionID: "s1",
            role: "assistant",
            createdAt: 0,
            finishedAt: 600_000,
            usage: { input: 10, output: 600, cacheRead: 0, cacheWrite: 0 },
            cost: 0,
            modelID: "m",
        },
        parts: [
            {
                id: "p1",
                messageID: "a1",
                sessionID: "s1",
                type: "reasoning",
                text: "r",
                time: { start: 0, end: 2_000 },
            },
            {
                id: "p2",
                messageID: "a1",
                sessionID: "s1",
                type: "tool_call",
                name: "task",
                status: "done",
                input: {},
                output: "",
                startedAt: 2_000,
                finishedAt: 590_000,
            },
            {
                id: "p3",
                messageID: "a1",
                sessionID: "s1",
                type: "text",
                text: "answer",
                time: { start: 590_000, end: 600_000 },
            },
        ],
    } as never;
    expect(generationMs(entry)).toBe(600_000);
    expect(tokensPerSecond(entry)).toBe("1.0");

    const openEnded = {
        info: {
            id: "a2",
            sessionID: "s1",
            role: "assistant",
            createdAt: 0,
            finishedAt: 20_000,
            usage: { input: 1, output: 100, cacheRead: 0, cacheWrite: 0 },
        },
        parts: [{ id: "p1", messageID: "a2", sessionID: "s1", type: "text", text: "t", time: { start: 10_000 } }],
    } as never;
    // An unterminated part falls back to the message completion time.
    expect(generationMs(openEnded)).toBe(0);
    expect(tokensPerSecond(openEnded)).toBe("5.0");
});

test("the code view paints its background on the scroller, not on the inner block", async () => {
    const [markup, css] = await Promise.all([
        Bun.file("src/ui/progressive-code.tsx").text(),
        Bun.file("src/styles/app.css").text(),
    ]);
    // The wrapper is what scrolls sideways, so only the wrapper's background covers the full width.
    expect(markup).toContainCode("code-view code-stream overflow-auto");
    expect(markup).toContainCode('classList={{ "max-h-80": !props.fill, "min-h-0 flex-1": props.fill }}');
    expect(css).toMatch(/\.code-view \{\s*background: var\(--raised\);/);
    expect(css).toMatch(/\.code-view :where\(pre\) \{\s*background: transparent !important;/);
    // A themed background has to move with it, otherwise the same seam reappears under that theme.
    expect(css).not.toContain(".code-view pre.shiki");
    expect(css).toContain('data-syntax-theme="dracula"] :where(.md pre.shiki, .code-view, .diff-view');
});
