import { defaultBase, matching, samePath, sameSource, sourceTitle } from "../src/ui/review-sources";
import { reviewRows } from "../src/ui/review-rows";
import { expect, test } from "bun:test";

const unchanged = (count: number, from = 1) => Array.from({ length: count }, (_, index) => ` line ${from + index}`);

test("unchanged runs beyond three lines of a change fold, and an opened fold shows its lines", () => {
    const lines = [...unchanged(10), "-old", "+new", ...unchanged(10, 11)];

    const rows = reviewRows(lines, new Set());
    expect(rows[0]).toEqual({ kind: "fold", start: 0, count: 7 });
    expect(rows.slice(1, 4).map((row) => row.kind)).toEqual(["ctx", "ctx", "ctx"]);
    expect(rows[4]).toEqual({ kind: "del", line: 11, text: "old", index: 10 });
    expect(rows[5]).toEqual({ kind: "add", line: 11, text: "new", index: 11 });
    expect(rows.at(-1)).toEqual({ kind: "fold", start: 15, count: 7 });

    const opened = reviewRows(lines, new Set([0]));
    expect(opened[0]).toEqual({ kind: "ctx", line: 1, text: "line 1", index: 0 });
    expect(opened.filter((row) => row.kind === "fold")).toHaveLength(1);
});

test("a short unchanged run between changes stays, and line numbers follow each side", () => {
    const lines = ["+a", ...unchanged(7), "-b", " tail"];

    const rows = reviewRows(lines, new Set());
    expect(rows.find((row) => row.kind === "fold")).toBeUndefined();
    expect(rows.at(-2)).toEqual({ kind: "del", line: 8, text: "b", index: 8 });
    expect(rows.at(-1)).toEqual({ kind: "ctx", line: 9, text: "tail", index: 9 });
});

test("one unchanged line is never folded into a bar of its own", () => {
    const rows = reviewRows(["+a", ...unchanged(3), " far"], new Set());

    expect(rows.find((row) => row.kind === "fold")).toBeUndefined();
});

test("All changes compares with the main line, and a branch comparison reads base to current", () => {
    const refs = {
        repo: true,
        branch: "next/2.3.0",
        branches: ["feat/x", "origin/master", "master"],
        commits: [{ id: "c1abc", short: "c1a", subject: "Fix", author: "Kyle", time: 0 }],
    };
    expect(defaultBase(refs)).toBe("master");
    expect(defaultBase({ ...refs, branches: ["feat/x"] })).toBe("feat/x");
    expect(defaultBase({ repo: false, branches: [], commits: [] })).toBeUndefined();
    expect(sourceTitle({ kind: "branch", target: "master" }, refs)).toEqual({ text: "master", to: "next/2.3.0" });
    expect(sourceTitle({ kind: "commit", target: "c1abc" }, refs).text).toBe("c1a Fix");

    const turn = { kind: "turn", sessionId: "ses", messageId: "msg" } as const;
    expect(sameSource(turn, { ...turn })).toBeTrue();
    expect(sameSource({ kind: "branch", target: "a" }, { kind: "branch", target: "b" })).toBeFalse();
    expect(sameSource({ kind: "uncommitted" }, { kind: "uncommitted" })).toBeTrue();
    expect(matching(["fix/streaming", "feat/splash"], "FIX str", (name) => name)).toEqual(["fix/streaming"]);
});

test("a workspace-relative review path matches the absolute path an edit names", () => {
    expect(samePath("src/a.ts", "C:\\Work\\Repo\\src\\A.ts")).toBeTrue();
    expect(samePath("src/a.ts", "src/a.ts")).toBeTrue();
    expect(samePath("a.ts", "C:/repo/data.ts")).toBeFalse();
});

test("a line note finds its line by file, side, number and text, and reaches the model as one text file", async () => {
    const { commentOn, reviewCommentsFile, reviewCommentsText } = await import("../src/state/review-comments");
    const note = {
        id: "n1",
        path: "src/api/mod.rs",
        line: 5,
        side: "add" as const,
        code: "let h = harness().await;",
        text: "use a fixture",
    };
    expect(commentOn(note, "src/api/mod.rs", { kind: "add", line: 5, text: "let h = harness().await;" })).toBeTrue();
    expect(commentOn(note, "src/api/mod.rs", { kind: "del", line: 5, text: "let h = harness().await;" })).toBeFalse();
    expect(commentOn(note, "src/api/lib.rs", { kind: "add", line: 5, text: "let h = harness().await;" })).toBeFalse();

    const text = reviewCommentsText([note]);
    expect(text).toContain("src/api/mod.rs line 5 (added):\n    let h = harness().await;\nuse a fixture");
    const file = reviewCommentsFile([{ ...note, text: "naïve ünïcode" }]);
    expect(file.mime).toBe("text/plain");
    const decoded = new TextDecoder().decode(Uint8Array.from(atob(file.url.split(",")[1]!), (c) => c.charCodeAt(0)));
    expect(decoded).toContain("naïve ünïcode");
});

test("a draft holding only line notes can be sent", async () => {
    const { createComposerSubmit } = await import("../src/ui/composer-submit");
    let sent = false;
    const draft = {
        text: "",
        staged: [],
        mentions: [],
        comments: [{ id: "n", path: "a", line: 1, side: "ctx" as const, code: "x", text: "y" }],
    };
    const submit = createComposerSubmit({
        scope: () => "s",
        session: () => "ses",
        workspace: () => ({ id: "w", name: "w", path: "C:/w" }),
        online: () => true,
        draft: () => draft,
        prepare: () => undefined,
        transform: async ({ text }) => text,
        newSession: async () => undefined,
        sessionScope: () => "s",
        migrateDraft: () => undefined,
        selectSession: () => undefined,
        sessionCreated: () => undefined,
        send: async () => {
            sent = true;
            return { ok: true };
        },
        admitted: () => undefined,
    });
    expect(await submit()).toBe("submitted");
    expect(sent).toBeTrue();
});
