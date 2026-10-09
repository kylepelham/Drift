import { indexSource, inspectSource } from "../scripts/inspect-claude-source";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { expect, test } from "bun:test";
import { tmpdir } from "node:os";
import * as ts from "typescript";
import path from "node:path";

const source = Buffer.from(
    '/*\xe9*/const alpha = () => { function repeated() { c("tengu_event", { secret: "do not copy" }); } function repeated2() { F8("tengu_switch", false); p5("tengu_gate"); oS("tengu_setting"); return process.env.ONE; } return repeated2; }; const beta = () => { function repeated() { return process.env.TWO; } return process.env["BRACKET"] ?? process.env[key] ?? process.env; };',
    "latin1",
);

test("AST index uses byte offsets, separate owners and syntactic callee groups", () => {
    const base = 71;
    const result = indexSource(ts, source, base);
    expect(result.summary.parseDiagnostics).toBe(0);
    expect(result.summary.tenguCalls).toBe(4);
    expect(result.summary.tenguByCallee.map(({ callee }) => callee).sort()).toEqual(["F8", "c", "oS", "p5"]);
    expect(result.summary.tenguCategories.candidateFlagAccessor.count).toBe(3);
    expect(result.summary.tenguCategories.telemetryEvent.count).toBe(1);
    expect(result.summary.directCallReferences).toBeGreaterThanOrEqual(4);
    expect(result.summary.directEnvAccesses).toBe(2);
    expect(result.summary.distinctDirectEnvNames).toBe(2);
    expect(result.summary.otherEnvAccesses).toBe(3);
    expect(result.environmentAccesses.map(({ kind }) => kind)).toEqual([
        "property",
        "property",
        "literal",
        "dynamic",
        "object",
    ]);
    const repeated = result.functions.filter((item) => item.name === "repeated");
    expect(repeated).toHaveLength(2);
    expect(repeated[0].parent).not.toBe(repeated[1].parent);
    expect(result.tenguCalls[0].offset).toBe(base + source.indexOf('c("tengu_event"'));
    expect(result.environmentAccesses[0].offset).toBe(base + source.indexOf("process.env.ONE"));
    expect(JSON.stringify(result)).not.toContain("do not copy");
});

test("parser reports syntax errors by byte offset without copying diagnostic source", () => {
    const bad = Buffer.from("const f = () => {", "latin1");
    const result = indexSource(ts, bad, 41);
    expect(result.summary.parseDiagnostics).toBeGreaterThan(0);
    expect(result.diagnostics[0].offset).toBeGreaterThanOrEqual(41);
    expect(Object.keys(result.diagnostics[0]).sort()).toEqual(["code", "length", "offset"]);
});

test("CLI requires a bounded explicit range and writes only to a fresh requested file", async () => {
    const directory = mkdtempSync(path.join(tmpdir(), "drift-source-index-"));
    try {
        const input = path.join(directory, "sample.bin");
        const output = path.join(directory, "index.json");
        writeFileSync(input, source);
        await expect(inspectSource(input, `0:${source.length + 1}`)).rejects.toThrow("Range must be inside");
        const tool = path.resolve(import.meta.dirname, "../scripts/inspect-claude-source.ts");
        const args = [tool, "--input", input, "--range", `0:${source.length}`, "--output", output];
        const run = spawnSync(process.execPath, args, { encoding: "utf8" });
        expect(run.status).toBe(0);
        const result = JSON.parse(readFileSync(output, "utf8"));
        expect(result.fileSha256).toBe(createHash("sha256").update(source).digest("hex"));
        expect(result.source.start).toBe(0);
        expect(result.source.end).toBe(source.length);
        expect(result.typescriptVersion).toBe(ts.version);
        expect(run.stdout).not.toContain("do not copy");
        expect(spawnSync(process.execPath, args, { encoding: "utf8" }).status).not.toBe(0);
    } finally {
        rmSync(directory, { recursive: true, force: true });
    }
});
