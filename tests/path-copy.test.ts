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
