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
