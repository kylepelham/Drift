import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const storage = new Map<string, string>()
localStorage.getItem = (key: string) => storage.get(key) ?? null
localStorage.setItem = (key: string, value: string) => storage.set(key, value)

test("cached threads survive a restart and only the engine may replace them", async () => {
  const { cachedSessions, cachedSessionLimit, forgetCachedSessions, rememberSessions } = await import(
    "../src/state/session-cache"
  )
  const directory = "C:/work/drift"
  expect(cachedSessions(directory)).toEqual([])

  rememberSessions(directory, [{ id: "one", title: "First", updated: 2 }])
  expect(cachedSessions(directory)).toEqual([{ id: "one", title: "First", updated: 2 }])
  expect(JSON.parse(localStorage.getItem("drift.sessions.cache")!)[directory]).toHaveLength(1)
  rememberSessions(directory, [{ id: "one", title: "First", updated: 3 }])
  expect(cachedSessions(directory)[0]?.updated).toBe(3)

  // An authoritative empty list clears the workspace instead of stranding deleted threads.
  rememberSessions(directory, [])
  expect(cachedSessions(directory)).toEqual([])

  const many = Array.from({ length: cachedSessionLimit + 10 }, (_, index) => ({
    id: `session-${index}`,
    title: `Thread ${index}`,
    updated: index,
  }))
  rememberSessions(directory, many)
  expect(cachedSessions(directory)).toHaveLength(cachedSessionLimit)

  forgetCachedSessions(directory)
  expect(cachedSessions(directory)).toEqual([])
})

const model = (id: string, name: string) => ({
  id,
  name,
  capabilities: { toolcall: true },
  limit: { context: 200_000 },
})

test("a cached provider catalog seeds engine state until fresh data overwrites it", async () => {
  const { cachedProviderCatalog, rememberProviderCatalog, seedProviderCatalog } = await import(
    "../src/state/provider-cache"
  )
  const { createEngineState, resolveModel } = await import("../src/engine/store")

  expect(cachedProviderCatalog()).toBeNull()
  rememberProviderCatalog(
    [{ id: "anthropic", name: "Anthropic", models: { sonnet: model("sonnet", "Sonnet") } }] as never,
    ["anthropic"],
    { anthropic: "sonnet" },
  )
  expect(JSON.parse(localStorage.getItem("drift.providers.cache")!).providers).toHaveLength(1)

  const [state, set] = createEngineState()
  expect(seedProviderCatalog(state, set)).toBe(true)
  expect(state.providers[0]?.id).toBe("anthropic")
  expect(state.connected).toEqual(["anthropic"])
  expect(state.defaultModels).toEqual({ anthropic: "sonnet" })
  // The saved preference resolves from the seeded catalog instead of falling back to "Default".
  expect(resolveModel(state, { providerID: "anthropic", modelID: "sonnet" })).toEqual({
    providerID: "anthropic",
    modelID: "sonnet",
  })

  // Fresh engine data always overwrites the seed, and a later seed call must not clobber it.
  set("providers", [{ id: "openai", name: "OpenAI", models: { "gpt-5": model("gpt-5", "GPT-5") } }] as never)
  set("connected", ["openai"])
  expect(seedProviderCatalog(state, set)).toBe(false)
  expect(state.providers[0]?.id).toBe("openai")
  rememberProviderCatalog(state.providers, state.connected, {})
  expect(cachedProviderCatalog()?.providers[0]?.id).toBe("openai")

  // The cache strips models to what the picker reads; engine-only payload fields are dropped.
  rememberProviderCatalog(
    [{ id: "openai", name: "OpenAI", models: { "gpt-5": { ...model("gpt-5", "GPT-5"), cost: { input: 1 } } } }] as never,
    [],
    {},
  )
  expect("cost" in (cachedProviderCatalog()?.providers[0]?.models["gpt-5"] ?? {})).toBe(false)
})

test("a corrupt provider catalog cache is discarded instead of seeding garbage", async () => {
  const { normalizeProviderCatalog } = await import("../src/state/provider-cache")
  expect(normalizeProviderCatalog(null)).toBeNull()
  expect(normalizeProviderCatalog("not a catalog")).toBeNull()
  expect(normalizeProviderCatalog(["wrong shape"])).toBeNull()
  expect(normalizeProviderCatalog({ providers: "nope" })).toBeNull()
  expect(normalizeProviderCatalog({ providers: [{ id: "broken" }] })).toBeNull()
  // Partially valid catalogs keep the well-formed entries and drop the rest.
  const normalized = normalizeProviderCatalog({
    providers: [
      { id: "anthropic", name: "Anthropic", models: { sonnet: model("sonnet", "Sonnet"), bad: { id: "bad" } } },
      { id: "empty", name: "Empty", models: {} },
      null,
    ],
    connected: ["anthropic", 7],
    defaultModels: { anthropic: "sonnet", broken: 3 },
  })
  expect(normalized?.providers).toHaveLength(1)
  expect(Object.keys(normalized?.providers[0]?.models ?? {})).toEqual(["sonnet"])
  expect(normalized?.connected).toEqual(["anthropic"])
  expect(normalized?.defaultModels).toEqual({ anthropic: "sonnet" })
})

test("a stored cache is restored and malformed entries are discarded", async () => {
  const { normalizeSessionCache, cachedSessionLimit } = await import("../src/state/session-cache")
  expect(normalizeSessionCache(null)).toEqual({})
  expect(normalizeSessionCache(["not a map"])).toEqual({})
  expect(
    normalizeSessionCache({
      "C:/work": [
        { id: "keep", title: "Keep", updated: 5 },
        { id: "missing-updated", title: "Broken" },
        { id: 7, title: "Wrong type", updated: 1 },
        null,
      ],
      "C:/empty": [],
    }),
  ).toEqual({ "C:/work": [{ id: "keep", title: "Keep", updated: 5 }] })
  expect(
    normalizeSessionCache({
      "C:/work": Array.from({ length: cachedSessionLimit + 5 }, (_, index) => ({
        id: `${index}`,
        title: `${index}`,
        updated: index,
      })),
    })["C:/work"],
  ).toHaveLength(cachedSessionLimit)
})
