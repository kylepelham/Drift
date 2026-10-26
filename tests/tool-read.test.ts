import { parseReadOutput } from "../src/ui/tool-read";
import { expect, test } from "bun:test";

test("a read's numbered lines are split from the paging note and reminders after them", () => {
    const page = parseReadOutput("160: uint local = e;\n161: \n162:     x();\n\n(18 more lines; read with offset 163)");

    expect(page.lines).toEqual([
        { line: 160, text: "uint local = e;" },
        { line: 161, text: "" },
        { line: 162, text: "    x();" },
    ]);
    expect(page.rest).toBe("(18 more lines; read with offset 163)");
    expect(parseReadOutput("(the file is empty)").lines).toEqual([]);
});

test("a line holding a carriage return or line separator stays one numbered line", () => {
    const page = parseReadOutput("1: 50%\r100%\n2: a\u2028b\n3: end\n\n(done)");

    expect(page.lines).toEqual([
        { line: 1, text: "50%100%" },
        { line: 2, text: "a\u2028b" },
        { line: 3, text: "end" },
    ]);
    expect(page.rest).toBe("(done)");
});
