import { expect, test } from "bun:test";

const kinds = { commands: true, edits: false, fetches: false, mcp: false, other: false };

let next = 0;
function tool(name: string, input: Record<string, unknown> = {}, extra: Record<string, unknown> = {}) {
    next++;
    return {
        id: `prt_${String(next).padStart(4, "0")}`,
        messageId: "m",
        sessionId: "s",
        type: "tool_call",
        callId: `call_${next}`,
        name,
        input,
        status: "done",
        output: "ok",
        ...extra,
    } as never;
}

function reply(id: string, parts: unknown[]) {
    return {
        info: { id, sessionId: "s", role: "assistant", status: "done", createdAt: 1, finishedAt: 2 },
        parts,
    } as never;
}

test("consecutive commands fold into one run across steps, and a lone command stays a plain row", async () => {
    const { groupAssistantEntries, groupParts } = await import("../src/ui/message-groups");
    const first = tool("bash", { command: "cargo test", description: "Run the engine tests" });
    const second = tool("bash", { command: "bun test" });
    const groups = groupAssistantEntries([reply("msg_1", [first]), reply("msg_2", [second])], kinds);

    expect(groups.get("msg_1")).toEqual([
        { id: `run:${(first as { id: string }).id}`, key: `run:${(first as { id: string }).id}`, run: [first, second] },
    ]);
    expect(groups.get("msg_2")).toEqual([]);
    expect(groupParts([first], kinds)).toEqual([
        { id: (first as { id: string }).id, key: (first as { id: string }).id, part: first },
    ]);
});

test("a kind the user did not group breaks a run, and enabled kinds merge into one row", async () => {
    const { groupParts } = await import("../src/ui/message-groups");
    const commands = [tool("bash", { command: "a" }), tool("bash", { command: "b" })];
    const edit = tool("edit", { filePath: "C:/w/a.ts" });

    const split = groupParts([commands[0], edit, commands[1]], kinds);
    expect(split.map((group) => ("part" in group ? "part" : "run"))).toEqual(["part", "part", "part"]);

    const merged = groupParts([commands[0], edit, commands[1]], { ...kinds, edits: true });
    expect(merged).toHaveLength(1);
    expect("run" in merged[0] && merged[0].run).toHaveLength(3);
});

test("a run's label counts each kind, names the latest command, and reads in the present while one runs", async () => {
    const { loadDictionary } = await import("../src/state/i18n");
    await loadDictionary("en");
    const { runDetail, runLongestMs, runSummary } = await import("../src/ui/tool-run");
    const parts = [
        tool("bash", { command: "cargo test", description: "Run the engine tests" }),
        tool("edit", { filePath: "C:/w/a.ts" }),
        tool("edit", { filePath: "C:/w/a.ts" }),
        tool("bash", { command: "bun run build" }),
    ];

    expect(runSummary(parts, false)).toBe("Ran 2 commands, edited 1 file");
    expect(runSummary(parts.slice(0, 1), true)).toBe("Running 1 command");
    expect(runDetail(parts)).toBe("");
    const timed = (startedAt: number, finishedAt: number, name = "bash") =>
        tool(name, { command: "x" }, { startedAt, finishedAt });
    expect(runLongestMs([timed(0, 2_000), timed(5_000, 607_000), timed(0, 900_000, "edit")])).toBe(602_000);
    expect(runLongestMs([timed(0, 3_000, "edit"), timed(0, 1_000, "webfetch")])).toBe(3_000);
    expect(runDetail([tool("bash", { command: "sleep 9", description: "Wait" }, { status: "running" })])).toBe("Wait");
});

test("tools are classed by grouping setting; delegation, questions and reads never join a run", async () => {
    const { toolGroupKind } = await import("../src/ui/tool-labels");
    expect(toolGroupKind("bash")).toBe("commands");
    expect(toolGroupKind("apply_patch")).toBe("edits");
    expect(toolGroupKind("webfetch")).toBe("fetches");
    expect(toolGroupKind("github_search_issues")).toBe("mcp");
    expect(toolGroupKind("process")).toBe("other");
    for (const name of ["task", "spawn_thread", "question", "read", "grep", "todowrite"])
        expect(toolGroupKind(name)).toBeUndefined();
});

test("the edited-files card lists a turn's files once with summed counts, its subagents' files by name", async () => {
    const { editedFiles, turnOf } = await import("../src/ui/edited-files");
    const diff = (add: number, del: number) =>
        [
            `@@ -1,${del} +1,${add} @@`,
            ...Array.from({ length: del }, () => "-old"),
            ...Array.from({ length: add }, () => "+new"),
        ].join("\n");
    const prompt = { info: { id: "msg_0", role: "user" }, parts: [] } as never;
    const steps = [
        reply("msg_1", [tool("edit", { filePath: "C:/w/a.ts" }, { metadata: { diff: diff(2, 1) } })]),
        reply("msg_2", [
            tool("write", { filePath: "C:\\w\\A.ts" }, { metadata: { diff: diff(1, 0) } }),
            tool("edit", { filePath: "C:/w/b.ts" }, { status: "error", metadata: { diff: diff(5, 5) } }),
        ]),
    ];
    const worker = [reply("msg_w", [tool("edit", { filePath: "C:/w/c.ts" }, { metadata: { diff: diff(0, 3) } })])];

    const turn = turnOf([prompt, ...steps], "msg_2");
    expect(turn.prompt).toBe(prompt);
    expect(turn.replies).toEqual(steps);

    const files = editedFiles(turn.replies, [{ name: "Fix paths", entries: worker }]);
    expect(files.map((file) => [file.name, file.additions, file.deletions, file.by])).toEqual([
        ["a.ts", 3, 1, undefined],
        ["c.ts", 0, 3, "Fix paths"],
    ]);
});
