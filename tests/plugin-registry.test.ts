import { sourceProblem, validSourceUrl } from "../src/state/registry-sources";
import { describe, expect, test } from "bun:test";
import {
    buildConfig,
    fieldText,
    fieldValue,
    installedPath,
    loadRegistries,
    matchesRegistryQuery,
    type ConfigField,
    type RegistryPlugin,
} from "../src/state/plugin-registry";

const list: ConfigField = { key: "test", label: "Test", type: "list", default: ["cargo", "test"] };
const flag: ConfigField = { key: "on", label: "On", type: "boolean", default: true };
const count: ConfigField = { key: "n", label: "N", type: "number", default: 5 };
const json: ConfigField = { key: "map", label: "Map", type: "json", default: { a: 1 } };

describe("plugin registry config fields", () => {
    test("typed values are read by type and the default stands in for nothing or nonsense", () => {
        expect(fieldValue(list, undefined)).toEqual(["cargo", "test"]);
        expect(fieldValue(list, "bun, test ,tests/a.ts")).toEqual(["bun", "test", "tests/a.ts"]);
        expect(fieldValue(list, "")).toEqual([]);
        expect(fieldValue(flag, "false")).toBe(false);
        expect(fieldValue(count, "12")).toBe(12);
        expect(fieldValue(count, "twelve")).toBe(5);
        expect(fieldValue(json, '{"b":2}')).toEqual({ b: 2 });
        expect(fieldValue(json, "{nope")).toEqual({ a: 1 });
    });

    test("text for an input round-trips a list and pretty-prints JSON", () => {
        expect(fieldText(list, ["a", "b"])).toBe("a, b");
        expect(fieldText(json, { a: 1 })).toBe('{\n  "a": 1\n}');
        expect(fieldText(flag, undefined)).toBe("");
    });

    test("the stored config holds every field from what was typed or its default", () => {
        expect(buildConfig([list, flag, count], { on: "false" })).toEqual({ test: ["cargo", "test"], on: false, n: 5 });
    });
});

describe("plugin registry sources", () => {
    const plugin = (id: string, extra: Partial<RegistryPlugin> = {}): RegistryPlugin => ({
        id,
        name: id,
        description: "",
        category: "safety",
        hooks: ["before-tool"],
        config: [],
        version: "0.1.0",
        author: "Drift",
        source: "",
        download: `https://example.com/${id}.wasm`,
        sha256: "00",
        size: 1,
        ...extra,
    });

    test("a user's source comes first, its plugins are named for it, duplicates by id are dropped, and a failing source is reported", async () => {
        const original = globalThis.fetch;
        globalThis.fetch = (async () =>
            new Response(JSON.stringify({ version: 1, plugins: [plugin("guard"), plugin("notify")] }))) as typeof fetch;
        // The user's sources are read by the engine, so they come through the fetch given rather than the browser's.
        const engineFetch = async (id: string) => {
            if (id === "acme")
                return { version: 1, plugins: [plugin("guard", { name: "Acme guard" }), plugin("acme-policy")] };
            throw new Error("could not fetch: 401 (a token may be needed)");
        };
        try {
            const loaded = await loadRegistries(
                [
                    { id: "acme", name: "Acme" },
                    { id: "broken", name: "Broken" },
                ],
                true,
                engineFetch,
            );
            expect(loaded.plugins.map((item) => item.id)).toEqual(["guard", "acme-policy", "notify"]);
            expect(loaded.plugins[0]!.name).toBe("Acme guard");
            expect(loaded.plugins[0]!.sourceName).toBe("Acme");
            expect(loaded.plugins[0]!.sourceId).toBe("acme");
            expect(loaded.plugins[2]!.sourceName).toBeUndefined();
            expect(loaded.failures).toEqual([
                { name: "Broken", error: "could not fetch: 401 (a token may be needed)" },
            ]);
        } finally {
            globalThis.fetch = original;
        }
    });

    test("search covers the source name and install paths are under plugins/", () => {
        expect(matchesRegistryQuery(plugin("x", { sourceName: "Acme" }), "acme")).toBeTrue();
        expect(matchesRegistryQuery(plugin("x"), "zzz")).toBeFalse();
        expect(installedPath("git-context")).toBe("plugins/git-context.wasm");
        expect(validSourceUrl("https://registry.example.com/plugins.json")).toBeTrue();
        expect(validSourceUrl("http://registry.example.com/plugins.json")).toBeFalse();
        expect(validSourceUrl("not a url")).toBeFalse();
        expect(sourceProblem("url", "http://intranet/registry.json", false)).toBe("http");
        expect(sourceProblem("url", "http://intranet/registry.json", true)).toBeUndefined();
        expect(sourceProblem("github", "https://github.com/acme/tools", false)).toBeUndefined();
        expect(sourceProblem("github", "https://gitlab.com/acme/tools", false)).toBe("github");
        expect(sourceProblem("azure_devops", "https://dev.azure.com/acme/Tools/_git/plugins", false)).toBeUndefined();
        expect(sourceProblem("azure_devops", "https://dev.azure.com/acme/Tools", false)).toBe("azure");
        expect(sourceProblem("folder", "\\fileserver\drift", false)).toBeUndefined();
    });
});
