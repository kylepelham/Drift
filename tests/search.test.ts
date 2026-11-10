import { expect, test } from "bun:test";

import type { MessageEntry } from "../src/engine/store";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined, removeItem: () => undefined },
    });

const session = (id: string, title: string, directory: string, updatedAt: number) => ({
    id,
    title,
    directory,
    updatedAt,
});
const workspace = (id: string, name: string, path: string) => ({ id, name, path });

test("title search ranks literal matches first and resolves the owning workspace", async () => {
    const { searchSessionNames } = await import("../src/state/session-search");
    const sessions = [
        session("a", "Vulkan renderer notes", "C:\\work\\app", 10),
        session("b", "Rewrite the vulkan swapchain", "C:/work/app", 30),
        session("c", "Talking about revulkanization", "C:\\work\\app", 40),
        session("d", "Unrelated", "C:\\work\\app", 50),
    ];
    const workspaces = [workspace("w1", "App", "C:\\work\\app")];

    const hits = searchSessionNames(sessions, workspaces, new Set(["b"]), "vulkan");
    // Prefix match, then word-boundary match, then a match buried inside another word.
    expect(hits.map((hit) => hit.sessionId)).toEqual(["a", "b", "c"]);
    expect(hits[0].workspaceName).toBe("App");
    // Slash direction and case must not stop a session mapping to its workspace.
    expect(hits[1].workspaceName).toBe("App");
    expect(hits[1].archived).toBeTrue();
    expect(hits[0].archived).toBeFalse();
});

test("title search ignores queries too short to be useful", async () => {
    const { searchSessionNames, sessionSearchReady } = await import("../src/state/session-search");
    const sessions = [session("a", "anything", "C:/work", 1)];
    expect(sessionSearchReady("a")).toBeFalse();
    expect(sessionSearchReady(" ab ")).toBeTrue();
    expect(searchSessionNames(sessions, [], new Set(), "a")).toEqual([]);
});

test("content search keeps only the newest answer when queries overtake each other", async () => {
    const { createSessionSearchRunner } = await import("../src/state/session-search");
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
        release = resolve;
    });
    const runner = createSessionSearchRunner({
        store: {
            searchSessions: async (query: string) => {
                if (query === "slow") await blocked;
                return [
                    {
                        sessionId: query,
                        messageId: "m1",
                        title: `${query} thread`,
                        directory: "C:/work",
                        updatedAt: 5,
                        excerpt: `about ${query}`,
                    },
                ];
            },
        },
        sessions: () => [],
        workspaces: () => [workspace("w1", "Work", "C:/work")],
        archived: () => new Set(),
    });

    const applied: string[] = [];
    const apply = (state: { hits?: { sessionId: string }[] }) => {
        if (state.hits) applied.push(state.hits.map((hit) => hit.sessionId).join(",") || "empty");
    };

    const slow = runner("slow", "content", apply);
    const fast = runner("fast", "content", apply);
    await fast;
    release();
    await slow;

    // The superseded query resolves last but must not overwrite the newer result.
    expect(applied).toEqual(["fast"]);
});

test("content hits map to workspaces and carry the matching message", async () => {
    const { contentHits } = await import("../src/state/session-search");
    const hits = contentHits(
        [
            {
                sessionId: "s1",
                messageId: "m9",
                title: "Old thread",
                directory: "C:\\work\\app",
                updatedAt: 7,
                excerpt: "…the vulkan swapchain…",
            },
            { sessionId: "s2", messageId: "", title: "Orphan", directory: "D:\\gone", updatedAt: 3, excerpt: "x" },
        ],
        [workspace("w1", "App", "C:/work/app")],
        new Set(["s1"]),
    );
    expect(hits[0]).toMatchObject({ workspaceId: "w1", workspaceName: "App", messageId: "m9", archived: true });
    // A workspace Drift no longer tracks still lists, labelled by its directory.
    expect(hits[1]).toMatchObject({ workspaceId: "", workspaceName: "D:\\gone", messageId: undefined });
});

test("excerpt highlighting splits every occurrence without building markup", async () => {
    const { highlightSegments } = await import("../src/state/session-search");
    expect(highlightSegments("The Vulkan and vulkan bits", "vulkan")).toEqual([
        { text: "The ", match: false },
        { text: "Vulkan", match: true },
        { text: " and ", match: false },
        { text: "vulkan", match: true },
        { text: " bits", match: false },
    ]);
    expect(highlightSegments("nothing", "")).toEqual([{ text: "nothing", match: false }]);
});

const entry = (id: string, parts: Record<string, unknown>[], role = "assistant"): MessageEntry =>
    ({
        info: { id, sessionId: "s1", role, createdAt: 1 },
        parts: parts.map((part, index) => ({ id: `${id}-p${index}`, messageId: id, sessionId: "s1", ...part })),
    }) as MessageEntry;

test("in-session search counts each part a row draws, the user's message whole", async () => {
    const { entrySearchText, transcriptMatches, totalMatches } = await import("../src/state/transcript-search");
    const entries = [
        entry("m1", [{ type: "text", text: "the cache is warm" }], "user"),
        entry("m2", [
            { type: "reasoning", text: "the cache might be cold" },
            {
                type: "tool_call",
                name: "bash",
                input: { command: "ls" },
                status: "done",
                output: "cache miss, cache miss",
            },
        ]),
        entry("m3", [{ type: "text", text: "unrelated" }]),
        // Drift's own scaffolding is not something the user ever saw.
        entry("m4", [{ type: "task_result", text: "cache" }]),
    ];

    // Reasoning counts only while it is shown, as it is drawn only then.
    expect(transcriptMatches(entries, "cache").map((match) => match.unit)).toEqual(["m1", "m2-p1"]);
    const { setShowReasoning } = await import("../src/state/prefs");
    setShowReasoning(true);
    const matches = transcriptMatches(entries, "cache");
    setShowReasoning(false);
    expect(matches).toEqual([
        { rowId: "m1", unit: "m1", messageId: "m1", count: 1 },
        { rowId: "m2", unit: "m2-p0", messageId: "m2", count: 1 },
        { rowId: "m2", unit: "m2-p1", messageId: "m2", count: 2 },
    ]);
    expect(totalMatches(matches)).toBe(4);
    expect(entrySearchText(entries[3])).toBe("");
    expect(transcriptMatches(entries, "  ")).toEqual([]);
});

test("a step drawn inside an earlier row is found in that row, and drawn counts replace the data's", async () => {
    const { firstOccurrenceIn, transcriptMatches, unitFingerprints } = await import("../src/state/transcript-search");
    const first = entry("a1", [{ type: "text", text: "needle" }]);
    const later = entry("a2", [
        { type: "tool_call", name: "bash", input: {}, status: "done", output: "needle needle" },
    ]);
    // The timeline drew a2's call in a1's group, so a2 has no row of its own.
    const partsOf = (row: MessageEntry) => (row.info.id === "a1" ? [...first.parts, ...later.parts] : []);
    const matches = transcriptMatches([first], "needle", partsOf);
    expect(matches.map((match) => [match.rowId, match.messageId, match.count])).toEqual([
        ["a1", "a1", 1],
        ["a1", "a2", 2],
    ]);
    expect(firstOccurrenceIn(matches, "a2")).toBe(1);

    // The row showed one occurrence where the data held two; the drawn count holds until the part changes.
    const fingerprint = unitFingerprints([first], partsOf).get("a2-p0")!;
    const overrides = new Map([["a2-p0", { fingerprint, count: 1 }]]);
    expect(transcriptMatches([first], "needle", partsOf, overrides)[1].count).toBe(1);
    (later.parts[0] as { output: string }).output = "needle needle needle";
    expect(transcriptMatches([first], "needle", partsOf, overrides)[1].count).toBe(3);
});

test("match navigation wraps at both ends and survives a growing transcript", async () => {
    const { occurrenceAt, reanchorMatch, stepMatch } = await import("../src/state/transcript-search");
    expect(stepMatch(0, 3, 1)).toBe(1);
    expect(stepMatch(2, 3, 1)).toBe(0);
    expect(stepMatch(0, 3, -1)).toBe(2);
    expect(stepMatch(0, 0, 1)).toBe(-1);

    // The cursor walks occurrences: repeats inside one unit are distinct stops.
    const match = (unit: string, count: number) => ({ rowId: unit, unit, messageId: unit, count });
    const matches = [match("m2", 2), match("m5", 1)];
    const at = (unit: string, index: number) => ({ rowId: unit, unit, messageId: unit, index });
    expect(occurrenceAt(matches, 0)).toEqual(at("m2", 0));
    expect(occurrenceAt(matches, 1)).toEqual(at("m2", 1));
    expect(occurrenceAt(matches, 2)).toEqual(at("m5", 0));
    expect(occurrenceAt(matches, 3)).toBeUndefined();
    expect(occurrenceAt(matches, -1)).toBeUndefined();

    // An older page loading in front of the cursor must not move the highlight to another unit.
    const after = [match("m1", 3), ...matches];
    expect(reanchorMatch(after, at("m5", 0), 0)).toBe(5);
    // The second occurrence in a unit stays the second occurrence after the set grows.
    expect(reanchorMatch(after, at("m2", 1), 0)).toBe(4);
    // An occurrence that disappeared clamps to the last one that still exists in its unit.
    expect(reanchorMatch(after, at("m2", 9), 0)).toBe(4);
    // A unit that stopped matching leaves the cursor on what came next, or going back, on what came before.
    expect(reanchorMatch(after, at("gone", 0), 3)).toBe(3);
    expect(reanchorMatch(after, at("gone", 0), 3, -1)).toBe(2);
    expect(reanchorMatch(after, at("gone", 0), 6)).toBe(0);
    expect(reanchorMatch([], at("m5", 0), 0)).toBe(-1);
});

test("the search text cache follows a part that is still streaming", async () => {
    const { transcriptMatches } = await import("../src/state/transcript-search");
    const streaming = entry("m1", [{ type: "text", text: "cache" }]);
    expect(transcriptMatches([streaming], "cache")[0].count).toBe(1);
    // The same object growing (a streamed reply) must invalidate the cached lowered text.
    (streaming.parts[0] as { text: string }).text = "cache then cache again";
    expect(transcriptMatches([streaming], "cache")[0].count).toBe(2);
});
