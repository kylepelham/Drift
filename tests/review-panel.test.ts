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

test("a pinned unchanged line stays shown with its context, as a change does", () => {
    const lines = [...unchanged(20), "+new"];

    const rows = reviewRows(lines, new Set(), new Set([5]));
    expect(rows[0]).toEqual({ kind: "fold", start: 0, count: 2 });
    expect(rows.find((row) => row.kind !== "fold" && row.index === 5)).toEqual({
        kind: "ctx",
        line: 6,
        text: "line 6",
        index: 5,
    });
    expect(rows.filter((row) => row.kind === "fold")).toHaveLength(2);
});

test("a finding anchors on its line, follows its text when the line moved, and is marked when it changed", async () => {
    const { anchorFinding } = await import("../src/state/review-findings");
    const lines = [" fn start() {", "-    old();", "+    let total = sum(items);", "+    log(total);", " }"];
    const finding = { file: "src/run.rs", summary: "sum can overflow", failureScenario: "large carts" };

    expect(anchorFinding({ ...finding, line: 2, code: "    let total = sum(items);" }, lines)).toEqual({
        index: 2,
        state: "exact",
    });
    expect(anchorFinding({ ...finding, line: 4, code: "    let total = sum(items);" }, lines)).toEqual({
        index: 2,
        state: "moved",
    });
    expect(anchorFinding({ ...finding, line: 3, code: "    total += 1;" }, lines)).toEqual({
        index: 3,
        state: "changed",
    });
    expect(anchorFinding({ ...finding, line: 40, code: "    log(total);" }, lines)?.state).toBe("moved");
    expect(anchorFinding({ ...finding, line: 1 }, lines)).toEqual({ index: 0, state: "exact" });
    expect(anchorFinding(finding, ["-gone"])).toBeUndefined();
    expect(anchorFinding({ ...finding, line: 1, code: "x".repeat(400) }, [`+${"x".repeat(500)}`])?.state).toBe("exact");
});

test("a later report's outcomes reach the findings it repeats, and Apply fixes lists what is open", async () => {
    const { findingOutcomes, findingReports, fixPrompt, isOutcomeReport } =
        await import("../src/state/review-findings");
    const call = (id: string, findings: object[], level?: string) => ({
        id,
        type: "tool_call",
        name: "report_findings",
        status: "done",
        metadata: { findings, level },
    });
    const first = [
        { file: "src/cart.ts", line: 12, summary: "Total ignores discounts", failureScenario: "a coupon applied" },
        { file: "src/cart.ts", line: 40, summary: "Empty cart divides by zero", failureScenario: "" },
        { file: "src/auth.ts", line: 3, summary: "Token compared with ==", failureScenario: "timing attack" },
    ];
    const entries = [
        { info: {}, parts: [call("p1", first, "high")] },
        {
            info: {},
            parts: [
                call("p2", [
                    { ...first[0], outcome: "fixed" },
                    { ...first[2], outcome: "skipped" },
                ]),
            ],
        },
        {
            info: {},
            parts: [
                call("p3", [
                    { ...first[1], summary: "Reworded", outcome: "no_change_needed" },
                    { ...first[2], outcome: "fixed" },
                ]),
            ],
        },
    ];

    const reports = findingReports(entries as never);
    expect(reports.map((report) => report.part.id)).toEqual(["p1", "p2", "p3"]);
    expect(reports[0].level).toBe("high");
    expect(reports[0].findings[1].id).toBe("p1:1");
    expect(findingOutcomes(reports, "p1")).toEqual(["fixed", "no_change_needed", "fixed"]);
    expect(isOutcomeReport(reports[1].findings)).toBeTrue();
    expect(isOutcomeReport(reports[0].findings)).toBeFalse();
    expect(isOutcomeReport([])).toBeFalse();

    const prompt = fixPrompt("Fix these.", [first[1]]);
    expect(prompt).toBe("Fix these.\n\n1. src/cart.ts:40 - Empty cart divides by zero");
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
