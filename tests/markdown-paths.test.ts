import { absolutePath } from "../src/ui/markdown-paths";
import { expect, test } from "bun:test";

const project = String.raw`C:\Users\Kyle\Desktop\C++\Drift`;
const windows = (path: string) => path.replaceAll("/", "\\");

test("a relative path in a project folder is copied whole, with the folder's separators and its line kept", () => {
    expect(absolutePath(windows("src-tauri/target/release/drift.exe"), project, [])).toBe(
        windows(`${project}/src-tauri/target/release/drift.exe`),
    );
    expect(absolutePath("src/ui/chat.tsx:120", project, [])).toBe(windows(`${project}/src/ui/chat.tsx:120`));
    expect(absolutePath(windows("D:/other/file.txt"), project, [])).toBe(windows("D:/other/file.txt"));
});

test("in a drive-wide folder a path the chat touched resolves to that file, the turn's own touches first", () => {
    const groups = [[windows(`${project}/src/ui/chat.tsx`)], [windows("C:/Work/old/src/ui/chat.tsx")]];

    expect(absolutePath("src/ui/chat.tsx", windows("C:/"), groups)).toBe(windows(`${project}/src/ui/chat.tsx`));
    expect(absolutePath("chat.tsx", windows("C:/"), groups)).toBe(windows(`${project}/src/ui/chat.tsx`));
    expect(absolutePath("src/ui/other.ts", windows("C:/"), groups)).toBe(windows("C:/src/ui/other.ts"));
});

test("names that are not touched files, commands and paths matching two touched files are copied as they read", () => {
    const groups = [[windows("C:/a/src/main.rs"), windows("C:/b/src/main.rs")]];

    expect(absolutePath("session.account", project, [])).toBeUndefined();
    expect(absolutePath("cargo test", project, [])).toBeUndefined();
    expect(absolutePath("src/main.rs", windows("C:/"), groups)).toBeUndefined();
    expect(absolutePath("src/ui/chat.tsx", undefined, [])).toBeUndefined();
    expect(absolutePath("crates/drift-engine/src/lib.rs", "/home/kyle/drift", [])).toBe(
        "/home/kyle/drift/crates/drift-engine/src/lib.rs",
    );
});
