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
    const failed = (id: string) => ({ ...tool("apply_patch", {}, { status: "error" }), id });
    expect(runSummary([failed("p1"), failed("p2")], false), "failed patches name no file but still count").toBe(
        "Edited 2 files",
    );
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
    const { editedFiles, turnOf, turnStartsEarlier } = await import("../src/ui/edited-files");
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
    expect(turnStartsEarlier(turn, "msg_0")).toBe(false);

    const paged = turnOf(steps, "msg_2");
    expect(paged.prompt).toBeUndefined();
    expect(turnStartsEarlier(paged, "msg_1"), "a turn longer than the loaded page").toBe(true);
    expect(turnStartsEarlier(paged, null), "the whole session is loaded").toBe(false);

    const files = editedFiles(turn.replies, [{ name: "Fix paths", entries: worker }]);
    expect(files.map((file) => [file.name, file.additions, file.deletions, file.by])).toEqual([
        ["a.ts", 3, 1, undefined],
        ["c.ts", 0, 3, "Fix paths"],
    ]);
});

test("every file icon a name can get has its SVG, so no row shows an empty image", async () => {
    const { fileIconName } = await import("../src/ui/file-icons");
    const source = await Bun.file("src/ui/file-icons.ts").text();
    const names = new Set([...source.matchAll(/:\s*"([a-z_]+)"/g)].map((match) => match[1]!));
    names.add(fileIconName("notes.unknown"));
    for (const name of names) expect(await Bun.file(`src/assets/file-icons/${name}.svg`).exists(), name).toBe(true);
    expect(fileIconName("src/ui/edited-files.ts")).toBe("typescript");
    expect(fileIconName("C:/repo/Cargo.toml")).toBe("rust");
});

test("a writing call is titled by what it does to its files, not by the tool's name", async () => {
    const { fileChangeKind } = await import("../src/ui/tool-labels");
    const { addedLines } = await import("../src/ui/tool-body");
    const call = (name: string, input: Record<string, unknown>, metadata: Record<string, unknown> = {}) =>
        ({ type: "tool_call", name, input, metadata, status: "done" }) as never;
    expect(fileChangeKind(call("write", { filePath: "a.ts" }, { created: true }))).toBe("create");
    expect(fileChangeKind(call("write", { filePath: "a.ts" }, { created: false }))).toBe("edit");
    expect(fileChangeKind(call("edit", { filePath: "a.ts" }))).toBe("edit");
    const patch = (type: string) => ({ filePath: "a.ts", type, patch: "", additions: 0, deletions: 0 });
    expect(fileChangeKind(call("apply_patch", {}, { fileChanges: [patch("add"), patch("add")] }))).toBe("create");
    expect(fileChangeKind(call("apply_patch", {}, { fileChanges: [patch("delete")] }))).toBe("delete");
    expect(fileChangeKind(call("apply_patch", {}, { fileChanges: [patch("add"), patch("update")] }))).toBe("edit");
    expect(fileChangeKind(call("apply_patch", { patchText: "*** Begin Patch\n*** Delete File: a.ts\n" }))).toBe(
        "delete",
    );
    expect(addedLines("one\ntwo\n")).toBe("@@ -0,0 +1,2 @@\n+one\n+two");
});

test("a background job's result or exit carries on the turn, so the edited files card waits for the turn's end", async () => {
    const { continuesTurn, turnOf } = await import("../src/ui/edited-files");
    const user = (id: string, parts: unknown[]) => ({ info: { id, role: "user" }, parts }) as never;
    const prompt = user("msg_0", [{ type: "text", text: "go" }]);
    const exit = user("msg_2", [{ type: "process_exit", processId: "p", code: 0 }]);
    const steps = [reply("msg_1", []), exit, reply("msg_3", [])];
    expect(continuesTurn(exit)).toBeTrue();
    expect(continuesTurn(prompt)).toBeFalse();
    expect(continuesTurn(user("msg_x", []))).toBeFalse();
    const turn = turnOf([prompt, ...steps], "msg_3");
    expect(turn.prompt).toBe(prompt);
    expect(turn.replies).toEqual(steps);
});
