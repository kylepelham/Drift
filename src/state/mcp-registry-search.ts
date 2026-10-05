import type {
  RegistryArgument,
  RegistryHeader,
  RegistryInput,
  RegistryListing,
  RegistryPackage,
  RegistryRemote,
  RegistryServer,
  RegistryVariable,
} from "../mcp-registry"

type FetchRegistry = (
  input: string,
  init: { signal: AbortSignal },
) => Promise<{
  ok: boolean
  json(): Promise<unknown>
}>

/** GitHub's curated registry: a few hundred servers ordered by stars, read whole and searched here. */
const GITHUB_REGISTRY = "https://api.mcp.github.com/v0.1/servers"
/** The official registry: thousands of entries, searched by name on the server for whatever GitHub's lacks. */
const OFFICIAL_REGISTRY = "https://registry.modelcontextprotocol.io/v0.1/servers"
const CATALOG_PAGES = 8
const CATALOG_TTL_MS = 6 * 60 * 60 * 1000

export type RegistrySearchResult = { stale: boolean; servers: RegistryServer[] }

export function parseRegistryPayload(value: unknown, source: RegistryListing["source"] = "official"): RegistryServer[] {
  const root = record(value)
  if (!root || !Array.isArray(root.servers)) throw new Error("The MCP Registry returned an invalid response")
  const unique = new Map<string, RegistryServer>()
  for (const entry of root.servers) {
    const wrapper = record(entry)
    if (!wrapper || !current(wrapper._meta)) continue
    const server = parseRegistryServer(wrapper.server ?? entry, source)
    if (server) unique.set(server.name, server)
  }
  return [...unique.values()]
}

function nextCursor(value: unknown) {
  const cursor = record(record(value)?.metadata)?.nextCursor
  return typeof cursor === "string" && cursor ? cursor : undefined
}

/** Entries the registry marks deleted or deprecated, or not the latest version, are not offered. */
function current(meta: unknown) {
  const official = record(record(meta)?.["io.modelcontextprotocol.registry/official"])
  if (!official) return true
  return official.status !== "deleted" && official.status !== "deprecated" && official.isLatest !== false
}

function parseRegistryServer(value: unknown, source: RegistryListing["source"]): RegistryServer | null {
  const item = record(value)
  if (!item) return null
  if (!text(item.name, 200) || !/^[A-Za-z0-9.-]+\/[A-Za-z0-9._-]+$/.test(item.name as string)) return null
  if (!text(item.description, 500) || !text(item.version, 255)) return null
  // One package or remote Drift cannot read leaves the others usable.
  const packages = lenientArray(item.packages, parsePackage)
  const remotes = lenientArray(item.remotes, parseRemote)
  const listing = parseListing(item, source)
  const title = text(item.title, 100) ? (item.title as string) : githubOf(item)?.displayName
  return {
    name: item.name as string,
    description: item.description as string,
    version: item.version as string,
    listing,
    ...(typeof title === "string" && text(title, 100) ? { title } : {}),
    ...(packages.length ? { packages } : {}),
    ...(remotes.length ? { remotes } : {}),
  }
}

function lenientArray<T>(value: unknown, parse: (entry: unknown) => T | null): T[] {
  return Array.isArray(value) ? value.slice(0, 128).flatMap((entry) => parse(entry) ?? []) : []
}

function githubOf(item: Record<string, unknown>) {
  return record(record(record(item._meta)?.["io.modelcontextprotocol.registry/publisher-provided"])?.github)
}

/** What a card shows: the publisher, a logo, stars and topics from GitHub when it knows the repository. */
function parseListing(item: Record<string, unknown>, source: RegistryListing["source"]): RegistryListing {
  const github = githubOf(item)
  const repository = httpsUrl(record(item.repository)?.url)
  const owner = typeof github?.nameWithOwner === "string" ? github.nameWithOwner.split("/")[0] : undefined
  const icon = Array.isArray(item.icons) ? httpsUrl(record(item.icons[0])?.src) : undefined
  const topics = Array.isArray(github?.topics) ? github.topics.filter((topic): topic is string => text(topic, 50)).slice(0, 12) : undefined
  return {
    source,
    publisher: owner ?? publisherOf(item.name as string),
    image: httpsUrl(github?.preferredImage) ?? httpsUrl(github?.ownerAvatarUrl) ?? icon,
    stars: typeof github?.stargazerCount === "number" ? github.stargazerCount : undefined,
    topics,
    repository,
    website: httpsUrl(item.websiteUrl),
  }
}

/** `io.github.acme/x` and `com.acme/x` are published by acme. */
function publisherOf(name: string) {
  const namespace = name.split("/")[0].split(".")
  if (namespace[0] === "io" && namespace[1] === "github") return namespace[2]
  return namespace.length > 1 ? namespace.at(-1 - (namespace.length > 2 ? 1 : 0)) : namespace[0]
}

/** How well a server matches every word of the query: title first, then name, publisher, topics and description; `0` when a word matches nothing. */
export function registryScore(server: RegistryServer, query: string) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  const title = (server.title ?? "").toLowerCase()
  const name = (server.name.split("/").at(-1) ?? "").toLowerCase()
  const publisher = (server.listing?.publisher ?? "").toLowerCase()
  const topics = (server.listing?.topics ?? []).map((topic) => topic.toLowerCase())
  const description = server.description.toLowerCase()
  let score = 0
  for (const word of words) {
    const points = Math.max(
      title === word ? 100 : title.startsWith(word) ? 70 : title.includes(word) ? 45 : 0,
      name.split(/[._-]/).includes(word) ? 50 : name.includes(word) ? 30 : 0,
      publisher === word ? 40 : publisher.includes(word) ? 20 : 0,
      topics.includes(word) ? 25 : topics.some((topic) => topic.includes(word)) ? 12 : 0,
      description.includes(word) ? 10 : 0,
    )
    if (!points) return 0
    score += points
  }
  return score
}

/** The servers that match, best first, popularity breaking ties; with no query, all of them by popularity. */
export function rankRegistry(servers: RegistryServer[], query: string) {
  const stars = (server: RegistryServer) => server.listing?.stars ?? 0
  if (!query.trim()) return [...servers].sort((a, b) => stars(b) - stars(a))
  return servers
    .map((server) => ({ server, score: registryScore(server, query) }))
    .filter((entry) => entry.score > 0)
    .sort((a, b) => b.score - a.score || stars(b.server) - stars(a.server))
    .map((entry) => entry.server)
}

let catalog: { at: number; servers: Promise<RegistryServer[]> } | undefined

/** Searches that end the one before them, so a slow answer never lands over a newer one. */
function latest() {
  let sequence = 0
  let controller: AbortController | undefined
  return {
    start() {
      const current = ++sequence
      controller?.abort()
      controller = new AbortController()
      return { signal: controller.signal, superseded: () => current !== sequence }
    },
    stop() {
      sequence++
      controller?.abort()
      controller = undefined
    },
  }
}

export function createRegistrySearch(fetchRegistry: FetchRegistry = fetch) {
  const local = latest()
  const remote = latest()
  const page = async (url: string, signal: AbortSignal) => {
    const response = await fetchRegistry(url, { signal })
    if (!response.ok) throw new Error("Could not load the MCP Registry")
    return response.json()
  }
  /** GitHub's whole list, shared by every search for a few hours. */
  const popular = () => {
    if (catalog && Date.now() - catalog.at < CATALOG_TTL_MS) return catalog.servers
    const loading = (async () => {
      const servers: RegistryServer[] = []
      let cursor: string | undefined
      for (let pages = 0; pages < CATALOG_PAGES; pages++) {
        const payload = await page(`${GITHUB_REGISTRY}?limit=100${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`, new AbortController().signal)
        servers.push(...parseRegistryPayload(payload, "github"))
        cursor = nextCursor(payload)
        if (!cursor) break
      }
      return servers
    })()
    catalog = { at: Date.now(), servers: loading }
    loading.catch(() => (catalog = undefined))
    return loading
  }
  return {
    /** GitHub's servers that match, from the list read once; fast after the first. */
    async search(query: string): Promise<RegistrySearchResult> {
      const { superseded } = local.start()
      const servers = rankRegistry(await popular(), query)
      return superseded() ? { stale: true, servers: [] } : { stale: false, servers }
    },
    /** The official registry's matches that `shown` lacks, by name or repository; it is slow, so it is asked separately. */
    async official(query: string, shown: RegistryServer[]): Promise<RegistrySearchResult> {
      const { signal, superseded } = remote.start()
      if (query.trim().length < 2) return { stale: false, servers: [] }
      const params = new URLSearchParams({ limit: "50", version: "latest", search: query.trim() })
      try {
        const found = parseRegistryPayload(await page(`${OFFICIAL_REGISTRY}?${params}`, signal), "official")
        const known = new Set(shown.flatMap((server) => [server.name, server.listing?.repository].filter(Boolean)))
        const servers = rankRegistry(found, query).filter((server) => !known.has(server.name) && !known.has(server.listing?.repository))
        return superseded() ? { stale: true, servers: [] } : { stale: false, servers }
      } catch (error) {
        if (superseded() || signal.aborted) return { stale: true, servers: [] }
        throw error
      }
    },
    dispose() {
      local.stop()
      remote.stop()
    },
  }
}

/** Forgets the shared list, so the next search reads it again. */
export function forgetRegistryCatalog() {
  catalog = undefined
}

function parsePackage(value: unknown): RegistryPackage | null {
  const item = record(value)
  const transport = record(item?.transport)
  if (!item || !transport || !text(transport.type, 40)) return null
  if (!text(item.registryType, 40) || !text(item.identifier, 500)) return null
  if (item.version !== undefined && !text(item.version, 255)) return null
  if (item.runtimeHint !== undefined && !text(item.runtimeHint, 40)) return null
  const runtimeArguments = optionalArray(item.runtimeArguments, parseArgument)
  const packageArguments = optionalArray(item.packageArguments, parseArgument)
  const environmentVariables = optionalArray(item.environmentVariables, parseVariable)
  if (runtimeArguments === null || packageArguments === null || environmentVariables === null) return null
  return {
    transport: { type: transport.type as string },
    registryType: item.registryType as string,
    identifier: item.identifier as string,
    version: typeof item.version === "string" ? item.version : "",
    ...(typeof item.runtimeHint === "string" ? { runtimeHint: item.runtimeHint } : {}),
    ...(runtimeArguments ? { runtimeArguments } : {}),
    ...(packageArguments ? { packageArguments } : {}),
    ...(environmentVariables ? { environmentVariables } : {}),
  }
}

function parseRemote(value: unknown): RegistryRemote | null {
  const item = record(value)
  if (!item || (item.type !== "streamable-http" && item.type !== "sse") || !text(item.url, 4096)) return null
  const headers = optionalArray(item.headers, parseHeader)
  const variables = optionalInputMap(item.variables)
  if (headers === null || variables === null) return null
  return {
    type: item.type,
    url: item.url as string,
    ...(headers ? { headers } : {}),
    ...(variables ? { variables } : {}),
  }
}

function parseArgument(value: unknown): RegistryArgument | null {
  const item = record(value)
  if (!item || (item.type !== "named" && item.type !== "positional")) return null
  if (item.type === "named" && !text(item.name, 256)) return null
  if (item.valueHint !== undefined && !text(item.valueHint, 256)) return null
  if (item.isRepeated !== undefined && typeof item.isRepeated !== "boolean") return null
  const input = parseInput(item)
  if (!input) return null
  return {
    ...input,
    type: item.type,
    ...(typeof item.name === "string" ? { name: item.name } : {}),
    ...(typeof item.valueHint === "string" ? { valueHint: item.valueHint } : {}),
    ...(typeof item.isRepeated === "boolean" ? { isRepeated: item.isRepeated } : {}),
  }
}

function parseVariable(value: unknown): RegistryVariable | null {
  const item = record(value)
  if (!item || !text(item.name, 256)) return null
  const input = parseInput(item)
  return input ? { ...input, name: item.name as string } : null
}

function parseHeader(value: unknown): RegistryHeader | null {
  return parseVariable(value)
}

function parseInput(value: Record<string, unknown>): RegistryInput | null {
  if (value.value !== undefined && !text(value.value, 16_384, true)) return null
  if (value.default !== undefined && !text(value.default, 16_384, true)) return null
  if (value.isRequired !== undefined && typeof value.isRequired !== "boolean") return null
  const variables = optionalInputMap(value.variables)
  if (variables === null) return null
  return {
    ...(typeof value.value === "string" ? { value: value.value } : {}),
    ...(typeof value.default === "string" ? { default: value.default } : {}),
    ...(text(value.description, 1000) ? { description: value.description as string } : {}),
    ...(typeof value.isRequired === "boolean" ? { isRequired: value.isRequired } : {}),
    ...(value.isSecret === true ? { isSecret: true } : {}),
    ...(variables ? { variables } : {}),
  }
}

function optionalInputMap(value: unknown): Record<string, RegistryInput> | undefined | null {
  if (value === undefined) return undefined
  const source = record(value)
  if (!source || Object.keys(source).length > 128) return null
  const result: Record<string, RegistryInput> = {}
  for (const [name, entry] of Object.entries(source)) {
    if (!/^[A-Za-z0-9._-]+$/.test(name)) return null
    const input = record(entry)
    const parsed = input && parseInput(input)
    if (!parsed) return null
    result[name] = parsed
  }
  return result
}

function optionalArray<T>(value: unknown, parse: (entry: unknown) => T | null): T[] | undefined | null {
  if (value === undefined) return undefined
  if (!Array.isArray(value) || value.length > 128) return null
  const result: T[] = []
  for (const entry of value) {
    const parsed = parse(entry)
    if (!parsed) return null
    result.push(parsed)
  }
  return result
}

function httpsUrl(value: unknown) {
  if (typeof value !== "string" || value.length > 2048) return undefined
  try {
    return new URL(value).protocol === "https:" ? value : undefined
  } catch {
    return undefined
  }
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined
}

function text(value: unknown, max: number, empty = false) {
  return typeof value === "string" && value.length <= max && (empty || value.length > 0) && !/[\0\r\n]/.test(value)
}
