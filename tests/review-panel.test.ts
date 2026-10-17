import { samePath, sourceFromId, sourceId, sourceItems } from "../src/ui/review-sources";
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

test("review sources round-trip through picker ids, and git ones are offered only in a repository", () => {
    const turn = { kind: "turn", sessionId: "ses", messageId: "msg" } as const;
    for (const source of [turn, { kind: "uncommitted" }, { kind: "branch", target: "origin/main" }] as const)
        expect(sourceFromId(sourceId(source), turn)).toEqual(source);
    expect(sourceFromId("commit:abc:def", undefined)).toEqual({ kind: "commit", target: "abc:def" });
    expect(sourceFromId("branch:", undefined)).toBeUndefined();

    expect(sourceItems({ repo: false, branches: [], commits: [] }, true).map((item) => item.id)).toEqual(["turn"]);
    const refs = {
        repo: true,
        branch: "work",
        branches: ["main"],
        commits: [{ id: "c1", short: "c1", subject: "Fix", author: "Kyle", time: 0 }],
    };
    expect(sourceItems(refs, false).map((item) => item.id)).toEqual(["uncommitted", "branch:main", "commit:c1"]);
});

test("a workspace-relative review path matches the absolute path an edit names", () => {
    expect(samePath("src/a.ts", "C:\\Work\\Repo\\src\\A.ts")).toBeTrue();
    expect(samePath("src/a.ts", "src/a.ts")).toBeTrue();
    expect(samePath("a.ts", "C:/repo/data.ts")).toBeFalse();
});
