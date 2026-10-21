import { nativePath, pathAt } from "../src/ui/path-copy";
import { expect, test } from "bun:test";

/** The path found when double-clicking the marked character (`|` marks the spot and is removed). */
function clicked(marked: string) {
    const offset = marked.indexOf("|");
    return pathAt(marked.replace("|", ""), offset)?.path;
}

test("a path is found whole from any character in it, without the sentence around it", () => {
    expect(clicked("see C:/Users/Kyle/Desk|top/C++/Drift/src/app.tsx for it")).toBe(
        "C:/Users/Kyle/Desktop/C++/Drift/src/app.tsx",
    );
    expect(clicked("edit `src/ui/mark|down.tsx`.")).toBe("src/ui/markdown.tsx");
    expect(clicked("(at crates/drift-engine/src/lib.rs:47)".replace("lib", "l|ib"))).toBe(
        "crates/drift-engine/src/lib.rs",
    );
    expect(clicked("log in /c/Users/Kyle/AppData/Lo|cal/Temp/x.log, then")).toBe(
        "/c/Users/Kyle/AppData/Local/Temp/x.log",
    );
    expect(clicked("~/.config/dr|ift/drift.json")).toBe("~/.config/drift/drift.json");
    expect(clicked("a folder: docs/rese|arch/")).toBe("docs/research/");
});

test("prose, numbers and web addresses are not paths, so a double-click selects a word as usual", () => {
    expect(clicked("and/|or")).toBeUndefined();
    expect(clicked("on 2026/10|/15")).toBeUndefined();
    expect(clicked("open https://github.com/kyle|pelham/Drift")).toBeUndefined();
    expect(clicked("just a wo|rd")).toBeUndefined();
});

test("a path is written with Windows separators on Windows and slashes elsewhere", () => {
    expect(nativePath("C:/Users/Kyle/Drift/src/app.tsx", true)).toBe("C:\\Users\\Kyle\\Drift\\src\\app.tsx");
    expect(nativePath("/c/Users/Kyle/AppData", true)).toBe("C:\\Users\\Kyle\\AppData");
    expect(nativePath("src/ui/markdown.tsx", true)).toBe("src\\ui\\markdown.tsx");
    expect(nativePath("src\\ui\\markdown.tsx", false)).toBe("src/ui/markdown.tsx");
    expect(nativePath("/home/kyle/drift", false)).toBe("/home/kyle/drift");
});

test("a path drawn as several pieces is found whole, and a new block is not joined onto it", async () => {
    const { pathAcross } = await import("../src/ui/path-copy");
    const highlighted = [
        { text: "Read ", newBlock: false },
        { text: "src/ui/", newBlock: false },
        { text: "composer", newBlock: false },
        { text: ".tsx", newBlock: false },
        { text: " (120 lines)", newBlock: false },
    ];
    const found = pathAcross(highlighted, { piece: 2, offset: 3 });
    expect(found?.path).toBe("src/ui/composer.tsx");
    expect(found?.start).toEqual({ piece: 1, offset: 0 });
    expect(found?.end).toEqual({ piece: 3, offset: 4 });

    const twoBlocks = [
        { text: "docs/engine.md", newBlock: false },
        { text: "notes/plan.md", newBlock: true },
    ];
    expect(pathAcross(twoBlocks, { piece: 0, offset: 3 })?.path).toBe("docs/engine.md");
});

test("a mention copies without its sign, and a code span keeps the spaces of a path but not of a command", async () => {
    const { wholePath } = await import("../src/ui/path-copy");
    expect(clicked("look at @src/ui/compo|ser.tsx")).toBe("src/ui/composer.tsx");
    const program = String.raw`C:\Program Files\Git\bin\bash.exe`;
    expect(wholePath(program)).toBe(program);
    expect(wholePath(" src/ui/app.tsx:40 ")).toBe("src/ui/app.tsx");
    expect(wholePath("src/ui")).toBe("src/ui");
    expect(wholePath("git checkout -- src/ui/app.tsx")).toBeUndefined();
    expect(wholePath("https://example.com/a/b")).toBeUndefined();
    expect(wholePath("Copy")).toBeUndefined();
});
