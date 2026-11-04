import { inputFields, outputView } from "../src/ui/tool-io";
import { expect, test } from "bun:test";

test("a call's arguments read as labelled rows, empty ones left out and nested ones kept as JSON", () => {
    const fields = inputFields({
        case_number: 38256,
        workItemId: 6541,
        no_cache: true,
        query: "",
        org: null,
        tags: ["a", "b"],
        gaps: ["a", null],
        filter: { state: "open" },
    });

    expect(fields.map(({ label, text, structured }) => [label, text, structured])).toEqual([
        ["Case number", "38256", false],
        ["Work item ID", "6541", false],
        ["No cache", "true", false],
        ["Tags", "a, b", false],
        ["Gaps", "a, null", false],
        ["Filter", '{\n  "state": "open"\n}', true],
    ]);
});

test("a result is shown as JSON, Markdown or text by what it holds", () => {
    expect(outputView('{"a":1}')).toEqual({ kind: "json", text: '{\n  "a": 1\n}' });
    expect(outputView("# FogBugz Case 38256\n**Status:** Closed").kind).toBe("markdown");
    expect(outputView("- one\n- two").kind).toBe("markdown");
    expect(outputView("OS     : Windows 11\nCPU    : i7").kind).toBe("text");
    expect(outputView("{ not json").kind).toBe("text");
    expect(outputView("  No to-do items found.\n")).toEqual({ kind: "text", text: "No to-do items found." });
});
