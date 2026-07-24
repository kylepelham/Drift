import type {
  RegistryArgument,
  RegistryHeader,
  RegistryInput,
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

export function registrySearchUrl(query: string) {
  const params = new URLSearchParams({ limit: "30", version: "latest" })
  if (query.trim()) params.set("search", query.trim())
  return `https://registry.modelcontextprotocol.io/v0.1/servers?${params}`
}

export function parseRegistryPayload(value: unknown): RegistryServer[] {
  const root = record(value)
  if (!root || !Array.isArray(root.servers)) throw new Error("The MCP Registry returned an invalid response")
  const unique = new Map<string, RegistryServer>()
  for (const entry of root.servers) {
    const wrapper = record(entry)
    const server = parseRegistryServer(wrapper?.server ?? entry)
    if (server) unique.set(server.name, server)
  }
  return [...unique.values()]
}

export function parseRegistryServer(value: unknown): RegistryServer | null {
  const item = record(value)
  if (!item) return null
  if (!text(item.name, 200) || !/^[A-Za-z0-9.-]+\/[A-Za-z0-9._-]+$/.test(item.name as string)) return null
  if (!text(item.description, 100) || !text(item.version, 255)) return null
  if (item.title !== undefined && !text(item.title, 100)) return null
  const packages = optionalArray(item.packages, parsePackage)
  const remotes = optionalArray(item.remotes, parseRemote)
  if (packages === null || remotes === null) return null
  return {
    name: item.name as string,
    description: item.description as string,
    version: item.version as string,
    ...(typeof item.title === "string" ? { title: item.title } : {}),
    ...(packages ? { packages } : {}),
    ...(remotes ? { remotes } : {}),
  }
}

export function createRegistrySearch(fetchRegistry: FetchRegistry = fetch) {
  let sequence = 0
  let controller: AbortController | undefined
  return {
    async search(query: string) {
      const current = ++sequence
      controller?.abort()
      controller = new AbortController()
      const signal = controller.signal
      let response: Awaited<ReturnType<FetchRegistry>>
      try {
        response = await fetchRegistry(registrySearchUrl(query), { signal })
      } catch (error) {
        if (current !== sequence || signal.aborted) return { stale: true, servers: [] as RegistryServer[] }
        throw error
      }
      if (current !== sequence) return { stale: true, servers: [] as RegistryServer[] }
      if (!response.ok) throw new Error("Could not load the official MCP Registry")
      const servers = parseRegistryPayload(await response.json())
      if (current !== sequence) return { stale: true, servers: [] as RegistryServer[] }
      return { stale: false, servers }
    },
    dispose() {
      sequence++
      controller?.abort()
      controller = undefined
    },
  }
}

function parsePackage(value: unknown): RegistryPackage | null {
  const item = record(value)
  const transport = record(item?.transport)
  if (!item || !transport || !text(transport.type, 40)) return null
  if (!text(item.registryType, 40) || !text(item.identifier, 500) || !text(item.version, 255)) return null
  if (item.runtimeHint !== undefined && !text(item.runtimeHint, 40)) return null
  const runtimeArguments = optionalArray(item.runtimeArguments, parseArgument)
  const packageArguments = optionalArray(item.packageArguments, parseArgument)
  const environmentVariables = optionalArray(item.environmentVariables, parseVariable)
  if (runtimeArguments === null || packageArguments === null || environmentVariables === null) return null
  return {
    transport: { type: transport.type as string },
    registryType: item.registryType as string,
    identifier: item.identifier as string,
    version: item.version as string,
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
    ...(typeof value.isRequired === "boolean" ? { isRequired: value.isRequired } : {}),
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

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined
}

function text(value: unknown, max: number, empty = false) {
  return typeof value === "string" && value.length <= max && (empty || value.length > 0) && !/[\0\r\n]/.test(value)
}
