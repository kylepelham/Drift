import { describe, expect, test } from "bun:test"
import { buildConfig, fieldText, fieldValue, installedPath, loadRegistries, matchesRegistryQuery, type ConfigField, type RegistryPlugin } from "../src/state/plugin-registry"
import { validSourceUrl } from "../src/state/registry-sources"

const list: ConfigField = { key: "test", label: "Test", type: "list", default: ["cargo", "test"] }
const flag: ConfigField = { key: "on", label: "On", type: "boolean", default: true }
const count: ConfigField = { key: "n", label: "N", type: "number", default: 5 }
const json: ConfigField = { key: "map", label: "Map", type: "json", default: { a: 1 } }

describe("plugin registry config fields", () => {
  test("typed values are read by type and the default stands in for nothing or nonsense", () => {
    expect(fieldValue(list, undefined)).toEqual(["cargo", "test"])
    expect(fieldValue(list, "bun, test ,tests/a.ts")).toEqual(["bun", "test", "tests/a.ts"])
    expect(fieldValue(list, "")).toEqual([])
    expect(fieldValue(flag, "false")).toBe(false)
    expect(fieldValue(count, "12")).toBe(12)
    expect(fieldValue(count, "twelve")).toBe(5)
    expect(fieldValue(json, '{"b":2}')).toEqual({ b: 2 })
    expect(fieldValue(json, "{nope")).toEqual({ a: 1 })
  })

  test("text for an input round-trips a list and pretty-prints JSON", () => {
    expect(fieldText(list, ["a", "b"])).toBe("a, b")
    expect(fieldText(json, { a: 1 })).toBe('{\n  "a": 1\n}')
    expect(fieldText(flag, undefined)).toBe("")
  })

  test("the stored config holds every field from what was typed or its default", () => {
    expect(buildConfig([list, flag, count], { on: "false" })).toEqual({ test: ["cargo", "test"], on: false, n: 5 })
  })
})

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
  })

  test("a user's source comes first, its plugins are named for it, duplicates by id are dropped, and a failing source is reported", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async (input: string | URL | Request) => {
      const url = String(input)
      if (url.includes("acme")) return new Response(JSON.stringify({ version: 1, plugins: [plugin("guard", { name: "Acme guard" }), plugin("acme-policy")] }))
      if (url.includes("broken")) return new Response("nope", { status: 500 })
      return new Response(JSON.stringify({ version: 1, plugins: [plugin("guard"), plugin("notify")] }))
    }) as typeof fetch
    try {
      const loaded = await loadRegistries([
        { name: "Acme", url: "https://acme.example/registry.json" },
        { name: "Broken", url: "https://broken.example/registry.json" },
      ], true)
      expect(loaded.plugins.map((item) => item.id)).toEqual(["guard", "acme-policy", "notify"])
      expect(loaded.plugins[0]!.name).toBe("Acme guard")
      expect(loaded.plugins[0]!.sourceName).toBe("Acme")
      expect(loaded.plugins[2]!.sourceName).toBeUndefined()
      expect(loaded.failures.map((failure) => failure.name)).toEqual(["Broken"])
    } finally {
      globalThis.fetch = original
    }
  })

  test("search covers the source name and install paths are under plugins/", () => {
    expect(matchesRegistryQuery(plugin("x", { sourceName: "Acme" }), "acme")).toBeTrue()
    expect(matchesRegistryQuery(plugin("x"), "zzz")).toBeFalse()
    expect(installedPath("git-context")).toBe("plugins/git-context.wasm")
    expect(validSourceUrl("https://registry.example.com/plugins.json")).toBeTrue()
    expect(validSourceUrl("http://registry.example.com/plugins.json")).toBeFalse()
    expect(validSourceUrl("not a url")).toBeFalse()
  })
})
