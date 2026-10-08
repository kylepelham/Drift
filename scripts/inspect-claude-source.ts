import { closeSync, fstatSync, openSync, readSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";

import type * as TypeScript from "typescript";

type TS = typeof TypeScript;
type Owner = {
    name: string;
    kind: string;
    offset: number;
    end: number;
    parent?: number;
    calls: { callee: string; offset: number }[];
};
type EnvAccess = { kind: "property" | "literal" | "dynamic" | "object"; offset: number; owner?: number; name?: string };
type TenguCall = { name: string; callee: string; offset: number; owner?: number };
const chunkSize = 1024 * 1024;
const maxRange = 16 * chunkSize;

function nameOf(ts: TS, node: TypeScript.FunctionLikeDeclaration) {
    const named = "name" in node ? node.name : undefined;
    if (named && ts.isIdentifier(named)) return named.text;
    const parent = node.parent;
    if (ts.isVariableDeclaration(parent) && ts.isIdentifier(parent.name)) return parent.name.text;
    if (ts.isPropertyAssignment(parent) && ts.isIdentifier(parent.name)) return parent.name.text;
    return "<anonymous>";
}

function calleeOf(ts: TS, expression: TypeScript.LeftHandSideExpression): string | undefined {
    if (ts.isIdentifier(expression)) return expression.text;
    if (!ts.isPropertyAccessExpression(expression)) return undefined;
    const base = calleeOf(ts, expression.expression as TypeScript.LeftHandSideExpression);
    if (!base || base.length + expression.name.text.length > 96) return undefined;
    return `${base}.${expression.name.text}`;
}

function isProcessEnv(ts: TS, node: TypeScript.Node) {
    return (
        ts.isPropertyAccessExpression(node) &&
        node.name.text === "env" &&
        ts.isIdentifier(node.expression) &&
        node.expression.text === "process"
    );
}

function bareProcessEnv(ts: TS, node: TypeScript.Node) {
    if (!isProcessEnv(ts, node) || !node.parent) return false;
    const parent = node.parent;
    if (ts.isPropertyAccessExpression(parent) || ts.isElementAccessExpression(parent))
        return parent.expression !== node;
    return true;
}

function bracketEnvName(ts: TS, node: TypeScript.ElementAccessExpression) {
    const argument = node.argumentExpression;
    if (!argument || !ts.isStringLiteral(argument)) return undefined;
    return /^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(argument.text) ? argument.text : undefined;
}

function envAccess(
    ts: TS,
    node: TypeScript.Node,
    root: TypeScript.SourceFile,
    base: number,
    owner?: Owner,
): EnvAccess | undefined {
    if (ts.isPropertyAccessExpression(node) && isProcessEnv(ts, node.expression)) {
        return { kind: "property", name: node.name.text, offset: base + node.getStart(root), owner: owner?.offset };
    }
    if (ts.isElementAccessExpression(node) && isProcessEnv(ts, node.expression)) {
        const name = bracketEnvName(ts, node);
        return {
            kind: name === undefined ? "dynamic" : "literal",
            name,
            offset: base + node.getStart(root),
            owner: owner?.offset,
        };
    }
    if (bareProcessEnv(ts, node)) {
        return { kind: "object", offset: base + node.getStart(root), owner: owner?.offset };
    }
    return undefined;
}

function functionWithBody(ts: TS, node: TypeScript.Node): node is TypeScript.FunctionLikeDeclaration {
    return ts.isFunctionLike(node) && "body" in node && node.body !== undefined;
}

function syntacticDiagnostics(ts: TS, root: TypeScript.SourceFile, base: number) {
    const options: TypeScript.CompilerOptions = { allowJs: true, noResolve: true, noLib: true };
    const host = ts.createCompilerHost(options);
    host.getSourceFile = (fileName) => (fileName === root.fileName ? root : undefined);
    host.fileExists = (fileName) => fileName === root.fileName;
    const program = ts.createProgram([root.fileName], options, host);
    return program
        .getSyntacticDiagnostics(root)
        .map((item) => ({ code: item.code, offset: base + (item.start ?? 0), length: item.length ?? 0 }));
}

function recordCall(
    ts: TS,
    node: TypeScript.CallExpression,
    root: TypeScript.SourceFile,
    base: number,
    owner: Owner | undefined,
    calls: TenguCall[],
    topLevelCalls: { callee: string; offset: number }[],
) {
    const callee = calleeOf(ts, node.expression);
    if (!callee) return;
    const offset = base + node.getStart(root);
    if (owner) owner.calls.push({ callee, offset });
    else topLevelCalls.push({ callee, offset });
    const first = node.arguments[0];
    if (first && ts.isStringLiteral(first) && /^tengu_[A-Za-z0-9_]+$/.test(first.text)) {
        calls.push({ name: first.text, callee, offset, owner: owner?.offset });
    }
}

function callGroups(calls: TenguCall[]) {
    const groups = new Map<string, { count: number; names: Set<string> }>();
    for (const call of calls) {
        const group = groups.get(call.callee) ?? { count: 0, names: new Set<string>() };
        group.count++;
        group.names.add(call.name);
        groups.set(call.callee, group);
    }
    return [...groups]
        .map(([callee, group]) => ({ callee, count: group.count, distinctNames: group.names.size }))
        .sort((a, b) => b.count - a.count || a.callee.localeCompare(b.callee));
}

function callCategories(calls: TenguCall[]) {
    const labels = ["candidateFlagAccessor", "telemetryEvent", "otherCallee"] as const;
    const categories = new Map(labels.map((label) => [label, { count: 0, names: new Set<string>() }]));
    for (const call of calls) {
        const label = callCategory(call.callee);
        const group = categories.get(label)!;
        group.count++;
        group.names.add(call.name);
    }
    return Object.fromEntries(
        labels.map((label) => [
            label,
            { count: categories.get(label)!.count, distinctNames: categories.get(label)!.names.size },
        ]),
    );
}

function callCategory(callee: string) {
    if (["F8", "p5", "oS"].includes(callee)) return "candidateFlagAccessor";
    if (callee === "c") return "telemetryEvent";

    return "otherCallee";
}

export function indexSource(ts: TS, data: Buffer, base: number) {
    const root = ts.createSourceFile(
        "bundle.js",
        data.toString("latin1"),
        ts.ScriptTarget.Latest,
        true,
        ts.ScriptKind.JS,
    );
    const functions: Owner[] = [];
    const topLevelCalls: { callee: string; offset: number }[] = [];
    const tenguCalls: TenguCall[] = [];
    const environmentAccesses: EnvAccess[] = [];
    let nodes = 0;
    let stringLiterals = 0;
    function visit(node: TypeScript.Node, owner?: Owner) {
        nodes++;
        if (ts.isStringLiteral(node)) stringLiterals++;
        if (functionWithBody(ts, node)) {
            owner = {
                name: nameOf(ts, node),
                kind: ts.SyntaxKind[node.kind],
                offset: base + node.getStart(root),
                end: base + node.end,
                parent: owner?.offset,
                calls: [],
            };
            functions.push(owner);
        }
        if (ts.isCallExpression(node)) recordCall(ts, node, root, base, owner, tenguCalls, topLevelCalls);
        const access = envAccess(ts, node, root, base, owner);
        if (access) environmentAccesses.push(access);
        ts.forEachChild(node, (child) => visit(child, owner));
    }
    visit(root);
    const diagnostics = syntacticDiagnostics(ts, root, base);
    const directEnv = environmentAccesses.filter((item) => item.kind === "property");
    const summary = {
        nodes,
        stringLiterals,
        functionCount: functions.length,
        parseDiagnostics: diagnostics.length,
        directCallReferences: topLevelCalls.length + functions.reduce((total, fn) => total + fn.calls.length, 0),
        tenguCalls: tenguCalls.length,
        distinctTenguNames: new Set(tenguCalls.map((call) => call.name)).size,
        tenguByCallee: callGroups(tenguCalls),
        tenguCategories: callCategories(tenguCalls),
        directEnvAccesses: directEnv.length,
        distinctDirectEnvNames: new Set(directEnv.map((item) => item.name)).size,
        otherEnvAccesses: environmentAccesses.length - directEnv.length,
    };
    return { summary, diagnostics, functions, topLevelCalls, tenguCalls, environmentAccesses };
}

function parseRange(value: string, size: number) {
    const match = /^(\d+):(\d+)$/.exec(value);
    if (!match) throw new Error("--range must be START:END in decimal bytes");
    const start = Number(match[1]);
    const end = Number(match[2]);
    if (
        !Number.isSafeInteger(start) ||
        !Number.isSafeInteger(end) ||
        end <= start ||
        end > size ||
        end - start > maxRange
    ) {
        throw new Error("Range must be inside the file and no larger than 16 MiB");
    }
    return { start, end };
}

function readRange(fd: number, start: number, end: number) {
    const data = Buffer.alloc(end - start);
    for (let read = 0; read < data.length;) {
        const count = readSync(fd, data, read, data.length - read, start + read);
        if (!count) throw new Error("Unexpected end of file");
        read += count;
    }
    return data;
}

function fileHash(fd: number, size: number) {
    const digest = createHash("sha256");
    for (let offset = 0; offset < size; offset += chunkSize) {
        digest.update(readRange(fd, offset, Math.min(size, offset + chunkSize)));
    }
    return digest.digest("hex");
}

export async function inspectSource(input: string, range: string) {
    const ts = await import("typescript");
    const fd = openSync(input, "r");
    try {
        const size = fstatSync(fd).size;
        const { start, end } = parseRange(range, size);
        const data = readRange(fd, start, end);
        return {
            file: path.basename(input),
            fileSize: size,
            fileSha256: fileHash(fd, size),
            source: { start, end, sha256: createHash("sha256").update(data).digest("hex") },
            typescriptVersion: ts.version,
            ...indexSource(ts, data, start),
        };
    } finally {
        closeSync(fd);
    }
}

function options(args: string[]) {
    const result: { input?: string; range?: string; output?: string } = {};
    const keys = new Set(["--input", "--range", "--output"]);
    for (let index = 0; index < args.length; index += 2) {
        const key = args[index];
        if (!keys.has(key) || !args[index + 1]) throw new Error(`Invalid argument: ${key}`);
        result[key.slice(2) as "input" | "range" | "output"] = args[index + 1];
    }
    if (!result.input || !result.range || !result.output)
        throw new Error("--input, --range, and --output are required");
    return result as { input: string; range: string; output: string };
}

if (import.meta.main) {
    try {
        if (process.argv.includes("--help")) {
            console.log(
                "Usage: bun scripts/inspect-claude-source.ts --input FILE --range START:END --output INDEX.json\nAll offsets are decimal, end exclusive. Range <=16 MiB. Output must not exist. Index stores structure and names, never source text or call arguments.",
            );
        } else {
            const args = options(process.argv.slice(2));
            const report = await inspectSource(args.input, args.range);
            writeFileSync(args.output, JSON.stringify(report) + "\n", { flag: "wx" });
            console.log(
                JSON.stringify({
                    fileSha256: report.fileSha256,
                    source: report.source,
                    typescriptVersion: report.typescriptVersion,
                    ...report.summary,
                }),
            );
        }
    } catch (error) {
        console.error(error instanceof Error ? error.message : error);
        process.exitCode = 1;
    }
}
